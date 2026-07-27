//! Window-scoped screen capture on Windows.
//!
//! `ScreenCapability::screenshot_window` was wired end to end in the tool layer
//! — `coord_space:"window"`, the window→global point mapping, the served image's
//! `coordinate_space` guide — and Windows inherited the trait's
//! `NotImplemented` default, so the whole path answered "not on this platform".
//! This is the missing limb.
//!
//! # Why `PrintWindow`, not a screen crop
//!
//! Cropping the desktop to a window's rectangle (what orca's Windows runtime
//! does with `CopyFromScreen`) captures whatever is *on top* of the window. That
//! is not the window, and the model cannot tell the difference — it reads
//! coordinates off pixels belonging to some other app.
//!
//! `PrintWindow` asks the window to render itself into a bitmap, so an occluded
//! or background window comes back intact and the user's foreground app is never
//! disturbed (R5). `PW_RENDERFULLCONTENT` is what makes that work for
//! DirectComposition-backed windows — Chromium, Electron, UWP and WPF — which
//! render nothing at all under the legacy flag.
//!
//! The fallback is a screen crop, kept only for the pre-composition or
//! refusing-provider case, and it is used *last* precisely because it can hand
//! back the wrong app's pixels.

#![cfg(windows)]

use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, Screenshot, WindowShot};

/// `PW_RENDERFULLCONTENT` — render the window through DWM rather than by
/// replaying `WM_PRINT`. Without it a Chromium/Electron/UWP window prints as a
/// blank rectangle. Windows 8.1+; older systems ignore the bit and fall through
/// to the blank-frame detection below.
const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;

/// Capture a single window, cropped to the frame a person sees.
///
/// The returned [`WindowShot`] carries the window's global-space origin, because
/// a crop whose pixels cannot be mapped back to click coordinates is a targeting
/// regression rather than a feature.
pub fn capture_window(window_id: u64, show_cursor: bool) -> Result<WindowShot> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindow};

    if show_cursor {
        // `PrintWindow` renders the window, not the desktop, so no cursor exists
        // in the frame to include. Saying so beats returning a cursor-free image
        // labelled as containing one.
        return Err(DesktopError::NotImplemented(
            "screenshot_window cannot draw the cursor on Windows: the window renders itself, and \
             the pointer is not part of it. Capture the display instead if the cursor matters."
                .into(),
        ));
    }

    let hwnd = HWND(window_id as usize as *mut core::ffi::c_void);

    // SAFETY: the handle is validated before any use; the remaining calls are
    // documented read-only Win32 queries.
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return Err(DesktopError::ScreenCapture(format!(
                "no window with id {window_id} is open"
            )));
        }
        if IsIconic(hwnd).as_bool() {
            return Err(DesktopError::ScreenCapture(format!(
                "window {window_id} is minimized; a minimized window has no live surface to \
                 capture. Use the desktop focus_window action first, or capture the display."
            )));
        }
    }

    let mut rect = RECT::default();
    // SAFETY: documented Win32 geometry read into a stack `RECT`.
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|e| DesktopError::ScreenCapture(format!("GetWindowRect failed: {e}")))?;
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    if width <= 0 || height <= 0 {
        return Err(DesktopError::ScreenCapture(format!(
            "window {window_id} has a zero-sized frame"
        )));
    }

    let pixels = print_window(hwnd, width, height)?;
    let full =
        image::RgbaImage::from_raw(width as u32, height as u32, pixels).ok_or_else(|| {
            DesktopError::ScreenCapture("captured buffer does not match the window size".into())
        })?;

    // `GetWindowRect` includes the invisible resize border DWM draws around a
    // composited window (~7 px per side), so the raw print carries a strip of
    // transparent padding. Crop to the frame a person sees — which is also the
    // rectangle `window_list` reports, so the pixels and the origin the model
    // maps them through describe the same rectangle.
    let visible = visible_frame(hwnd);
    let cropped = match visible.as_ref() {
        Some(frame) => crop_to_frame(&full, &rect, frame),
        None => image::DynamicImage::ImageRgba8(full),
    };

    let (out_w, out_h) = (cropped.width(), cropped.height());
    let mut buf = Cursor::new(Vec::new());
    cropped
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let window_bounds = visible.or(Some(BoundingBox {
        x: f64::from(rect.left),
        y: f64::from(rect.top),
        w: f64::from(width),
        h: f64::from(height),
    }));

    Ok(WindowShot {
        image: Screenshot {
            image_base64: general_purpose::STANDARD.encode(buf.into_inner()),
            width: out_w,
            height: out_h,
            format: "png".to_string(),
            scale_factor: Some(scale_for(window_bounds.as_ref())),
        },
        window_bounds,
    })
}

/// The DWM extended frame bounds, i.e. the window without its invisible border.
fn visible_frame(hwnd: windows::Win32::Foundation::HWND) -> Option<BoundingBox> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};

    let mut rect = RECT::default();
    // SAFETY: writes a `RECT` into `rect`; the size argument matches.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::addr_of_mut!(rect).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;

    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    (w > 0 && h > 0).then(|| BoundingBox {
        x: f64::from(rect.left),
        y: f64::from(rect.top),
        w: f64::from(w),
        h: f64::from(h),
    })
}

/// Crop the printed window bitmap down to `frame`, expressed in global pixels.
///
/// Pure geometry over two rectangles the OS reported; clamped so a frame that
/// (impossibly) reaches outside the print can only shrink the crop, never index
/// past the buffer.
fn crop_to_frame(
    full: &image::RgbaImage,
    window_rect: &windows::Win32::Foundation::RECT,
    frame: &BoundingBox,
) -> image::DynamicImage {
    use image::GenericImageView as _;

    let dx = (frame.x - f64::from(window_rect.left)).max(0.0) as u32;
    let dy = (frame.y - f64::from(window_rect.top)).max(0.0) as u32;
    let w = (frame.w as u32).min(full.width().saturating_sub(dx));
    let h = (frame.h as u32).min(full.height().saturating_sub(dy));
    if w == 0 || h == 0 {
        return image::DynamicImage::ImageRgba8(full.clone());
    }
    image::DynamicImage::ImageRgba8(full.view(dx, dy, w, h).to_image())
}

/// Pixels per point for the display this window sits on.
///
/// Under the per-monitor DPI awareness the platform opts into at construction
/// this is 1.0 — window geometry and captured pixels are the same numbers. It is
/// read from the display list rather than assumed, so a process that could not
/// take the opt-in still reports the ratio its geometry actually carries.
fn scale_for(bounds: Option<&BoundingBox>) -> f64 {
    let Ok(displays) = super::list_displays() else {
        return 1.0;
    };
    let containing = bounds.and_then(|b| {
        displays.iter().find(|d| {
            let (dx, dy) = (f64::from(d.origin_x), f64::from(d.origin_y));
            b.x >= dx
                && b.y >= dy
                && b.x < dx + f64::from(d.width)
                && b.y < dy + f64::from(d.height)
        })
    });
    containing
        .or_else(|| displays.iter().find(|d| d.is_primary))
        .or_else(|| displays.first())
        .map_or(1.0, |d| d.scale_factor)
}

/// Render `hwnd` into a top-down BGRA buffer and return it as RGBA.
///
/// Tries `PrintWindow(PW_RENDERFULLCONTENT)` first — the only variant that
/// captures an occluded or background window, including the
/// DirectComposition-backed ones (Chromium, Electron, UWP, WPF). A blank result
/// means the window refused to print; only then does it fall back to reading the
/// same rectangle off the screen, which is correct only when nothing overlaps
/// the window.
fn print_window(
    hwnd: windows::Win32::Foundation::HWND,
    width: i32,
    height: i32,
) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        SRCCOPY,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    // SAFETY: every GDI object created below is released on every path; the
    // buffer handed to `GetDIBits` is sized from the same width/height as the
    // bitmap and the header describes exactly that layout.
    unsafe {
        // A null HWND asks for the whole-screen DC, which is the reference
        // device the memory bitmap must be compatible with.
        let screen_dc = GetDC(HWND::default());
        if screen_dc.is_invalid() {
            return Err(DesktopError::ScreenCapture("GetDC failed".into()));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            ReleaseDC(HWND::default(), screen_dc);
            return Err(DesktopError::ScreenCapture(
                "CreateCompatibleDC failed".into(),
            ));
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND::default(), screen_dc);
            return Err(DesktopError::ScreenCapture(
                "CreateCompatibleBitmap failed".into(),
            ));
        }
        let previous = SelectObject(mem_dc, bitmap);

        let printed = PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();

        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative height requests a top-down DIB, matching the row
                // order `image::RgbaImage` expects.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut read = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            std::ptr::addr_of_mut!(info),
            DIB_RGB_COLORS,
        );

        // A window that refuses to print yields a fully transparent bitmap
        // rather than an error. Fall back to lifting the same rectangle off the
        // screen — correct only when nothing overlaps the window, which is why
        // it is the last resort and not the first.
        if !printed || read == 0 || is_blank(&pixels) {
            let mut rect = windows::Win32::Foundation::RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok()
                && BitBlt(
                    mem_dc, 0, 0, width, height, screen_dc, rect.left, rect.top, SRCCOPY,
                )
                .is_ok()
            {
                read = GetDIBits(
                    mem_dc,
                    bitmap,
                    0,
                    height as u32,
                    Some(pixels.as_mut_ptr().cast()),
                    std::ptr::addr_of_mut!(info),
                    DIB_RGB_COLORS,
                );
            }
        }

        SelectObject(mem_dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);

        if read == 0 {
            return Err(DesktopError::ScreenCapture(
                "GetDIBits returned no scanlines for the window".into(),
            ));
        }

        Ok(bgra_to_rgba(pixels))
    }
}

/// True when nothing was written into the bitmap at all — every byte still
/// zero, which is what a window that declined to print leaves behind.
///
/// Deliberately **not** "every pixel is transparent": GDI does not maintain the
/// alpha channel on the classic drawing path, so a perfectly good print of a
/// Win32 window comes back with real colour and alpha 0 everywhere. Testing
/// alpha alone would call almost every successful print blank and send it down
/// the screen-crop fallback — which captures whatever is *on top* of the window.
/// A genuinely all-black window also reads as blank here; it then takes the
/// fallback and is captured correctly anyway, so the false positive costs a
/// slower path, never wrong pixels.
///
/// Short-circuits on the first non-zero byte, so a healthy 4K print costs one
/// comparison rather than a full scan.
fn is_blank(bgra: &[u8]) -> bool {
    bgra.iter().all(|&b| b == 0)
}

/// Convert a GDI BGRA buffer to RGBA, repairing the alpha channel only when GDI
/// left it meaningless.
///
/// Two capture paths land here and they disagree about alpha. `PrintWindow` on a
/// DWM-composited window (Chromium, Electron, UWP, WPF) produces a real alpha
/// channel — opaque interior, transparent rounded corners — and that alpha
/// should be kept. The classic GDI path does not maintain alpha at all, so a
/// perfectly good print of a Win32 window arrives with correct colour and alpha
/// 0 in every pixel; honouring *that* would encode a fully invisible PNG.
///
/// The two are distinguishable exactly: a wholly-zero alpha channel is the GDI
/// case (a real capture is never uniformly transparent — it would be a window
/// with nothing in it), and only then is opacity forced.
fn bgra_to_rgba(mut buf: Vec<u8>) -> Vec<u8> {
    let alpha_is_meaningless = buf.chunks_exact(4).all(|px| px[3] == 0);
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        if alpha_is_meaningless {
            px[3] = 255;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_detection_means_nothing_was_written_not_nothing_is_opaque() {
        // Untouched bitmap: every byte zero.
        assert!(is_blank(&[0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(is_blank(&[]));
        // The regression this pins: GDI leaves alpha at 0 on a *successful*
        // print of a classic Win32 window. Colour with zero alpha is a real
        // capture, and treating it as blank would send it down the screen-crop
        // fallback — which captures whatever is on top of the window instead.
        assert!(!is_blank(&[12, 34, 56, 0]));
        assert!(!is_blank(&[0, 0, 0, 0, 12, 34, 56, 0]));
        // A composited window with meaningful alpha is obviously not blank.
        assert!(!is_blank(&[0, 0, 0, 255]));
    }

    #[test]
    fn a_wholly_zero_alpha_channel_is_repaired_to_opaque() {
        // GDI's usual "no alpha maintained": B=1, G=2, R=3, A=0.
        let out = bgra_to_rgba(vec![1, 2, 3, 0]);
        assert_eq!(
            out,
            vec![3, 2, 1, 255],
            "channels swapped and forced opaque"
        );
    }

    #[test]
    fn a_meaningful_alpha_channel_survives() {
        // A composited window: opaque interior, transparent rounded corner.
        // Forcing opacity there would paint the corner with whatever the
        // uninitialized bitmap happened to hold.
        let out = bgra_to_rgba(vec![1, 2, 3, 255, 4, 5, 6, 0]);
        assert_eq!(out, vec![3, 2, 1, 255, 6, 5, 4, 0]);
    }

    #[test]
    fn capture_rejects_a_bogus_handle() {
        // HWND 1 is never a valid window; the limb must say so rather than
        // return a frame of nothing.
        let err = capture_window(1, false).expect_err("bogus handle must error");
        assert!(matches!(err, DesktopError::ScreenCapture(_)), "{err:?}");
    }

    #[test]
    fn capture_refuses_to_pretend_it_can_draw_a_cursor() {
        let err = capture_window(1, true).expect_err("show_cursor is unsupported");
        assert!(matches!(err, DesktopError::NotImplemented(_)), "{err:?}");
    }
}
