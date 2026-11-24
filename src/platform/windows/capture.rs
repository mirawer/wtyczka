use anyhow::{Context, Result, ensure};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use std::{
    ffi::c_void,
    io::{self, Write},
    mem::{size_of, zeroed},
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::Semaphore, task::spawn_blocking, time::timeout};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
        CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, GetMonitorInfoW,
        HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow, ReleaseDC,
        SRCCOPY, SelectObject,
    },
    System::{
        StationsAndDesktops::{
            CloseDesktop, DESKTOP_READOBJECTS, GetProcessWindowStation, GetThreadDesktop,
            GetUserObjectInformationW, HDESK, OpenInputDesktop, UOI_NAME,
        },
        Threading::GetCurrentThreadId,
    },
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        },
        WindowsAndMessaging::GetForegroundWindow,
    },
};

const MAX_IMAGE: usize = 4 * 1024 * 1024;
// Includes 8K UHD, while bounding both the DIB and the RGB allocation.
const MAX_PIXELS: usize = 40_000_000;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
// A timed-out spawn_blocking cannot be killed. Keep its permit until it exits,
// so repeated timeouts cannot accumulate blocked GDI workers or large images.
static CAPTURE_SLOT: Semaphore = Semaphore::const_new(1);

/// Capture the foreground window's whole monitor, without the cursor, at native
/// physical resolution. No files, clipboard, picker, or protected-content bypass.
pub async fn capture() -> Result<Vec<u8>> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(cancelled.clone());
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    timeout(CAPTURE_TIMEOUT, async move {
        let permit = CAPTURE_SLOT
            .acquire()
            .await
            .context("Capture worker unavailable.")?;
        spawn_blocking(move || {
            let _permit = permit;
            let check_deadline = || -> Result<()> {
                ensure!(
                    !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
                    "Screen capture cancelled or timed out."
                );
                Ok(())
            };
            check_deadline()?;
            let frame = capture_frame(&check_deadline)?;
            check_deadline()?;
            let jpeg = encode_jpeg(&frame)?;
            check_deadline()?;
            // Also refuse if the session switched desktops while encoding.
            check_environment()?;
            Ok(jpeg)
        })
        .await
        .context("Screen capture worker failed.")?
    })
    .await
    .context("Screen capture exceeded the 15 second timeout.")?
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Fail closed unless this thread is on the normal interactive input desktop.
/// Never switch desktops, request elevated rights, or open the secure desktop.
pub fn check_environment() -> Result<()> {
    unsafe {
        let station = GetProcessWindowStation(); // Borrowed, not owned.
        ensure!(!station.is_null(), "No process window station.");
        ensure!(
            user_object_name(station)?.eq_ignore_ascii_case("WinSta0"),
            "Screen capture requires the interactive window station."
        );
        let thread = GetThreadDesktop(GetCurrentThreadId()); // Borrowed.
        ensure!(
            !thread.is_null(),
            "Cannot identify the current thread desktop."
        );
        let input = InputDesktop(OpenInputDesktop(0, 0, DESKTOP_READOBJECTS));
        ensure!(
            !input.0.is_null(),
            "Input desktop unavailable; the session may be locked or secure."
        );
        // Desktop handles from separate opens need not have equal values.
        // Names identify desktops within this process's window station.
        let thread_name = user_object_name(thread)?;
        let input_name = user_object_name(input.0)?;
        ensure!(
            thread_name.eq_ignore_ascii_case("Default")
                && input_name.eq_ignore_ascii_case(&thread_name),
            "Refusing capture on a switched, locked, or secure desktop."
        );
    }
    Ok(())
}

unsafe fn user_object_name(object: HANDLE) -> Result<String> {
    // Desktop and window-station names fit in this buffer; failure is closed.
    let mut name = [0u16; 512];
    let mut needed = 0u32;
    let ok = unsafe {
        GetUserObjectInformationW(
            object,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            size_of_val(&name) as u32,
            &mut needed,
        )
    };
    ensure!(ok != 0, "Cannot identify the input desktop/window station.");
    let bytes = needed as usize;
    ensure!(
        bytes >= 2 && bytes <= size_of_val(&name) && bytes.is_multiple_of(2),
        "Invalid desktop/window-station name."
    );
    let units = bytes / 2;
    ensure!(
        name[units - 1] == 0,
        "Unterminated desktop/window-station name."
    );
    String::from_utf16(&name[..units - 1]).context("Invalid desktop/window-station name.")
}

struct InputDesktop(HDESK);
impl Drop for InputDesktop {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseDesktop(self.0) };
        }
    }
}

// SetThreadDpiAwarenessContext requires Windows 10 1607+. Supported targets
// are contemporary Windows 10 (22H2) and Windows 11; no process-global DPI change.
struct DpiContext(DPI_AWARENESS_CONTEXT);
impl DpiContext {
    fn physical_pixels() -> Result<Self> {
        unsafe {
            let mut previous =
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // Per-monitor V1 also disables coordinate virtualization on older
            // Windows 10 versions which do not accept the V2 context.
            if previous.is_null() {
                previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
            }
            ensure!(
                !previous.is_null(),
                "Cannot enable physical-pixel capture coordinates."
            );
            Ok(Self(previous))
        }
    }
}
impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(null_mut(), self.0) };
    }
}
struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe { DeleteDC(self.0) };
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0) };
    }
}

struct Surface {
    // Fields drop in order after our Drop: delete the DC before the bitmap,
    // even if restoring the original selection unexpectedly fails.
    dc: MemoryDc,
    _bitmap: Bitmap,
    previous: HGDIOBJ,
    bits: *mut c_void,
}
impl Surface {
    fn new(screen: HDC, geometry: Geometry) -> Result<Self> {
        unsafe {
            let dc = MemoryDc(CreateCompatibleDC(screen));
            ensure!(!dc.0.is_null(), "Cannot create capture DC.");
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: geometry.width,
                    // Top-down BGRA, with naturally aligned 32-bit rows.
                    biHeight: -geometry.height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: u32::try_from(geometry.bgra_len)?,
                    ..zeroed()
                },
                ..zeroed()
            };
            let mut bits = null_mut();
            let bitmap = Bitmap(CreateDIBSection(
                screen,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                null_mut(),
                0,
            ));
            ensure!(
                !bitmap.0.is_null() && !bits.is_null(),
                "Cannot allocate capture bitmap."
            );
            let previous = SelectObject(dc.0, bitmap.0);
            ensure!(
                !previous.is_null() && previous as isize != -1,
                "Cannot select capture bitmap."
            );
            Ok(Self {
                dc,
                _bitmap: bitmap,
                previous,
                bits,
            })
        }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { SelectObject(self.dc.0, self.previous) };
    }
}

#[derive(Clone, Copy, Debug)]
struct Geometry {
    width: i32,
    height: i32,
    bgra_len: usize,
}
impl Geometry {
    fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Result<Self> {
        let width = right.checked_sub(left).context("Monitor width overflow.")?;
        let height = bottom
            .checked_sub(top)
            .context("Monitor height overflow.")?;
        ensure!(width > 0 && height > 0, "Monitor has invalid dimensions.");
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .context("Monitor pixel count overflow.")?;
        ensure!(
            pixels <= MAX_PIXELS,
            "Monitor exceeds the capture pixel limit."
        );
        let bgra_len = pixels
            .checked_mul(4)
            .context("Monitor buffer size overflow.")?;
        ensure!(
            bgra_len <= isize::MAX as usize,
            "Monitor buffer is too large."
        );
        Ok(Self {
            width,
            height,
            bgra_len,
        })
    }
}

struct Frame {
    geometry: Geometry,
    rgb: Vec<u8>,
}

fn capture_frame(check_deadline: &impl Fn() -> Result<()>) -> Result<Frame> {
    check_environment()?;
    let _dpi = DpiContext::physical_pixels()?;
    unsafe {
        let foreground = GetForegroundWindow();
        ensure!(!foreground.is_null(), "No foreground window to capture.");
        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONULL);
        ensure!(
            !monitor.is_null(),
            "Foreground window has no active monitor."
        );
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        ensure!(
            GetMonitorInfoW(monitor, &mut info) != 0,
            "Cannot query foreground monitor."
        );
        // rcMonitor, not rcWork: include taskbars and retain negative origins.
        let rect = info.rcMonitor;
        let geometry = Geometry::from_edges(rect.left, rect.top, rect.right, rect.bottom)?;
        let screen = ScreenDc(GetDC(null_mut()));
        ensure!(!screen.0.is_null(), "Cannot access the screen DC.");
        let surface = Surface::new(screen.0, geometry)?;
        check_deadline()?;
        check_environment()?;
        ensure!(
            BitBlt(
                surface.dc.0,
                0,
                0,
                geometry.width,
                geometry.height,
                screen.0,
                rect.left,
                rect.top,
                SRCCOPY | CAPTUREBLT,
            ) != 0,
            "GDI screen capture failed."
        );
        // A DIB's CPU memory must not be read until GDI has finished writing.
        ensure!(GdiFlush() != 0, "Cannot synchronize the capture bitmap.");
        check_environment()?;
        check_deadline()?;
        // The live DIB owns precisely this checked, top-down 32-bit buffer.
        let bgra = std::slice::from_raw_parts(surface.bits.cast::<u8>(), geometry.bgra_len);
        let rgb = bgra_to_rgb(bgra)?;
        ensure!(
            rgb.iter().any(|&channel| channel != 0),
            "Capture is entirely black; desktop may be unavailable or protected."
        );
        Ok(Frame { geometry, rgb })
    }
}

fn bgra_to_rgb(bgra: &[u8]) -> Result<Vec<u8>> {
    ensure!(bgra.len().is_multiple_of(4), "Incomplete BGRA pixel.");
    let len = (bgra.len() / 4)
        .checked_mul(3)
        .context("RGB buffer size overflow.")?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(len)
        .context("Cannot allocate RGB capture buffer.")?;
    for pixel in bgra.chunks_exact(4) {
        // BI_RGB's reserved alpha byte is not meaningful; do not composite it.
        rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    Ok(rgb)
}

struct BoundedJpeg(Vec<u8>);
impl Write for BoundedJpeg {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_IMAGE - self.0.len() {
            return Err(io::Error::other("JPEG exceeds the 4 MiB limit."));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_jpeg(frame: &Frame) -> Result<Vec<u8>> {
    let mut output = BoundedJpeg(Vec::new());
    output
        .0
        .try_reserve_exact(MAX_IMAGE)
        .context("Cannot allocate JPEG output buffer.")?;
    JpegEncoder::new_with_quality(&mut output, 85)
        .encode(
            &frame.rgb,
            frame.geometry.width as u32,
            frame.geometry.height as u32,
            ExtendedColorType::Rgb8,
        )
        .context("Cannot encode native-resolution JPEG at quality 85 (maximum 4 MiB).")?;
    Ok(output.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_accepts_negative_monitor_origins_and_native_8k() {
        let g = Geometry::from_edges(-3840, -2160, 0, 0).unwrap();
        assert_eq!(
            (g.width, g.height, g.bgra_len),
            (3840, 2160, 3840 * 2160 * 4)
        );
        assert!(Geometry::from_edges(0, 0, 7680, 4320).is_ok());
    }

    #[test]
    fn geometry_rejects_empty_reversed_overflowing_and_excessive_rects() {
        for edges in [
            (0, 0, 0, 10),
            (0, 0, 10, 0),
            (1, 0, 0, 10),
            (0, 1, 10, 0),
            (i32::MIN, 0, i32::MAX, 1),
            (0, i32::MIN, 1, i32::MAX),
            (0, 0, i32::MAX, i32::MAX),
            (0, 0, 10000, 10000),
        ] {
            assert!(Geometry::from_edges(edges.0, edges.1, edges.2, edges.3).is_err());
        }
    }

    #[test]
    fn conversion_swaps_channels_ignores_alpha_and_preserves_top_down_rows() {
        // Two rows of three pixels: 32-bit DIB rows require no extra padding,
        // even for odd widths. Alpha zero must not make the pixel transparent.
        let bgra = [
            3, 2, 1, 0, 6, 5, 4, 255, 9, 8, 7, 12, 12, 11, 10, 0, 15, 14, 13, 0, 18, 17, 16, 255,
        ];
        assert_eq!(bgra_to_rgb(&bgra).unwrap(), (1u8..=18).collect::<Vec<_>>());
        assert_eq!(bgra_to_rgb(&[0, 0, 0, 255]).unwrap(), [0, 0, 0]);
        assert!(bgra_to_rgb(&[1, 2, 3]).is_err());
    }

    #[test]
    fn jpeg_writer_enforces_exact_size_limit_without_partial_overflow() {
        let mut output = BoundedJpeg(vec![0; MAX_IMAGE - 1]);
        output.write_all(&[1]).unwrap();
        assert_eq!(output.0.len(), MAX_IMAGE);
        assert!(output.write_all(&[2]).is_err());
        assert_eq!(output.0.len(), MAX_IMAGE);
        assert_eq!(output.write(&[]).unwrap(), 0);
    }

    #[test]
    fn jpeg_has_native_dimensions() {
        let frame = Frame {
            geometry: Geometry::from_edges(0, 0, 3, 2).unwrap(),
            rgb: vec![127; 3 * 2 * 3],
        };
        let encoded = encode_jpeg(&frame).unwrap();
        assert!(encoded.starts_with(&[0xff, 0xd8, 0xff]));
        assert!(encoded.len() <= MAX_IMAGE);
        // Decoder is enabled by the same image/jpeg feature as the encoder.
        let decoded =
            image::load_from_memory_with_format(&encoded, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
    }
}
