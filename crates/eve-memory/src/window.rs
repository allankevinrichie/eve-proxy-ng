//! Top-level window operations for game clients.
//!
//! Supports multi-client scenarios: activate a specific client's window as
//! the foreground window, minimize or restore it, query its state, read its
//! client-area rectangle in screen coordinates, and capture the client area
//! to an image (used by the dump recorder for before/after screenshots).

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, HBITMAP, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindow, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_MINIMIZE, SW_RESTORE,
};

use crate::error::{Error, Result};

/// A rectangle in screen coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

/// A top-level window handle of a game client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WindowHandle(pub HWND);

// HWND is a kernel handle value, not a thread-bound resource; carrying it
// across threads (e.g. recorder capture/observe threads) is sound.
unsafe impl Send for WindowHandle {}
unsafe impl Sync for WindowHandle {}

impl WindowHandle {
    pub fn from_raw(raw: isize) -> Option<WindowHandle> {
        if raw == 0 {
            None
        } else {
            Some(WindowHandle(HWND(raw as *mut _)))
        }
    }

    pub fn raw(&self) -> isize {
        self.0 .0 as isize
    }

    pub fn is_valid(&self) -> bool {
        unsafe { IsWindow(Some(self.0)).as_bool() }
    }

    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.0).as_bool() }
    }

    pub fn is_minimized(&self) -> bool {
        unsafe { IsIconic(self.0).as_bool() }
    }

    pub fn title(&self) -> String {
        unsafe {
            let length = GetWindowTextLengthW(self.0);
            if length <= 0 {
                return String::new();
            }
            let mut buffer = vec![0u16; (length + 1) as usize];
            let copied = GetWindowTextW(self.0, &mut buffer);
            let end = copied.max(0) as usize;
            String::from_utf16_lossy(&buffer[..end.min(buffer.len())])
        }
    }

    /// Restore (if minimized) and bring the window to the foreground.
    /// Returns whether the foreground request was accepted by the shell.
    pub fn activate(&self) -> bool {
        unsafe {
            if IsIconic(self.0).as_bool() {
                let _ = ShowWindow(self.0, SW_RESTORE);
            }
            SetForegroundWindow(self.0).as_bool()
        }
    }

    pub fn minimize(&self) -> bool {
        unsafe { ShowWindow(self.0, SW_MINIMIZE).as_bool() }
    }

    pub fn restore(&self) -> bool {
        unsafe { ShowWindow(self.0, SW_RESTORE).as_bool() }
    }

    /// Client-area rectangle translated to screen coordinates.
    pub fn client_rect_screen(&self) -> Result<ScreenRect> {
        unsafe {
            let mut client = RECT::default();
            if GetClientRect(self.0, &mut client).is_err() {
                return Err(Error::WindowOperationFailed {
                    message: "GetClientRect failed".into(),
                });
            }
            let mut origin = POINT::default();
            if !ClientToScreen(self.0, &mut origin).as_bool() {
                return Err(Error::WindowOperationFailed {
                    message: "ClientToScreen failed".into(),
                });
            }
            Ok(ScreenRect {
                x: origin.x,
                y: origin.y,
                width: client.right - client.left,
                height: client.bottom - client.top,
            })
        }
    }

    /// Capture the client area from the screen (the window must be visible,
    /// not occluded for a faithful capture, and not minimized).
    pub fn capture_client_area(&self) -> Result<image::RgbImage> {
        unsafe {
            // Per-monitor-v2 awareness keeps coordinates physical, so DPI
            // virtualization never rescales them behind our back.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let rect = self.client_rect_screen()?;
            if rect.is_empty() {
                return Err(Error::CaptureFailed {
                    message: format!("empty client area {rect:?}"),
                });
            }
            let width = rect.width as i32;
            let height = rect.height as i32;

            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return Err(Error::CaptureFailed {
                    message: "GetDC failed".into(),
                });
            }
            let result = (|| -> Result<image::RgbImage> {
                let mem_dc = CreateCompatibleDC(Some(screen_dc));
                let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
                let old = SelectObject(mem_dc, bitmap.into());
                let blit = BitBlt(
                    mem_dc,
                    0,
                    0,
                    width,
                    height,
                    Some(screen_dc),
                    rect.x,
                    rect.y,
                    SRCCOPY | CAPTUREBLT,
                );
                if blit.is_err() {
                    return Err(Error::CaptureFailed {
                        message: "BitBlt failed".into(),
                    });
                }
                let mut info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        // Negative height requests a top-down row order.
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bgra = vec![0u8; (width * height * 4) as usize];
                let lines = GetDIBits(
                    mem_dc,
                    HBITMAP(bitmap.0),
                    0,
                    height as u32,
                    Some(bgra.as_mut_ptr() as *mut _),
                    &mut info,
                    DIB_RGB_COLORS,
                );
                SelectObject(mem_dc, old);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(mem_dc);
                if lines == 0 {
                    return Err(Error::CaptureFailed {
                        message: "GetDIBits failed".into(),
                    });
                }
                let mut rgb = Vec::with_capacity((width * height * 3) as usize);
                for pixel in bgra.chunks_exact(4) {
                    rgb.push(pixel[2]); // R
                    rgb.push(pixel[1]); // G
                    rgb.push(pixel[0]); // B
                }
                image::RgbImage::from_raw(width as u32, height as u32, rgb).ok_or_else(|| {
                    Error::CaptureFailed {
                        message: "buffer/size mismatch".into(),
                    }
                })
            })();
            ReleaseDC(None, screen_dc);
            result
        }
    }
}

/// Enumerate all visible top-level windows that have a title.
/// Returns `(handle, pid, title)` triples in Z-order (topmost first).
pub fn enumerate_titled_windows() -> Vec<(WindowHandle, u32, String)> {
    struct Collector {
        items: Vec<(WindowHandle, u32, String)>,
    }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        unsafe {
            let collector = &mut *(lparam.0 as *mut Collector);
            if IsWindowVisible(hwnd).as_bool() {
                let length = GetWindowTextLengthW(hwnd);
                if length > 0 {
                    let mut buffer = vec![0u16; (length + 1) as usize];
                    let copied = GetWindowTextW(hwnd, &mut buffer);
                    let end = (copied.max(0) as usize).min(buffer.len());
                    let title = String::from_utf16_lossy(&buffer[..end]);
                    let mut pid = 0u32;
                    windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                        hwnd, Some(&mut pid),
                    );
                    collector.items.push((WindowHandle(hwnd), pid, title));
                }
            }
            windows::core::BOOL(1)
        }
    }
    let mut collector = Collector { items: Vec::new() };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
            Some(callback),
            LPARAM(&mut collector as *mut _ as isize),
        );
    }
    collector.items
}
