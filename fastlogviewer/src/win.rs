//! The Win32 niceties the C# build reached for via P/Invoke: a themed title bar tinted to match the
//! toolbar, Windows 11 rounded corners, and the window icon pulled out of `shell32.dll`.
//!
//! Per-monitor-v2 DPI awareness is *not* here — winit already declares it and iced drives its
//! scale factor from it, which is what `Native.EnableDpiAwareness` was buying. Every call below
//! degrades to a no-op rather than failing the app, same as the original's swallowed exceptions.

use iced::window::Icon;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC,
};
use windows::Win32::UI::Shell::ExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// `DWMWA_USE_IMMERSIVE_DARK_MODE` on 1903+, and its 1809 predecessor.
const DWMWA_DARK_MODE: u32 = 20;
const DWMWA_DARK_MODE_1809: u32 = 19;
/// `DWMWA_WINDOW_CORNER_PREFERENCE`; 2 is `DWMWCP_ROUND`.
const DWMWA_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;
/// Windows 11 caption fill and caption text colours, as `COLORREF` (0x00BBGGRR).
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;

fn to_hwnd(handle: u64) -> HWND {
    HWND(handle as *mut core::ffi::c_void)
}

fn colorref(color: iced::Color) -> u32 {
    let [r, g, b, _] = color.into_rgba8();
    (b as u32) << 16 | (g as u32) << 8 | r as u32
}

/// Dark or light title bar (20 on 1903+, 19 on 1809), tinted to `caption` on Windows 11, and
/// rounded corners. Older Windows ignores the attributes it doesn't know.
pub fn apply_chrome(handle: u64, dark: bool, caption: iced::Color, caption_text: iced::Color) {
    let hwnd = to_hwnd(handle);
    let dark = dark as u32;
    unsafe {
        let set = |attr: u32, value: &u32| {
            DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(attr as i32),
                value as *const u32 as *const core::ffi::c_void,
                4,
            )
        };
        if set(DWMWA_DARK_MODE, &dark).is_err() {
            let _ = set(DWMWA_DARK_MODE_1809, &dark);
        }
        let round = DWMWCP_ROUND;
        let _ = set(DWMWA_CORNER_PREFERENCE, &round);
        let _ = set(DWMWA_CAPTION_COLOR, &colorref(caption));
        let _ = set(DWMWA_TEXT_COLOR, &colorref(caption_text));
    }
}

/// Extracts icon `index` from a DLL and converts it to the RGBA buffer iced wants for the window
/// icon. Returns `None` on any failure — the original threw here, which would have taken the whole
/// app down over a decoration.
pub fn load_dll_icon(dll_path: &str, index: u32) -> Option<Icon> {
    let wide: Vec<u16> = dll_path.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let hicon: HICON = ExtractIconW(None, PCWSTR(wide.as_ptr()), index);
        // ExtractIconW signals "no icons" with 1 and "failed" with 0.
        if hicon.0.is_null() || hicon.0 as usize == 1 {
            return None;
        }
        let icon = icon_to_rgba(hicon);
        let _ = DestroyIcon(hicon);
        icon
    }
}

unsafe fn icon_to_rgba(hicon: HICON) -> Option<Icon> {
    let mut info = ICONINFO::default();
    GetIconInfo(hicon, &mut info).ok()?;

    // Both bitmaps are ours to free once we are done reading the colour plane.
    let cleanup = |info: &ICONINFO| {
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
    };

    let mut bitmap = BITMAP::default();
    let read = GetObjectW(
        info.hbmColor.into(),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bitmap as *mut BITMAP as *mut core::ffi::c_void),
    );
    if read == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        cleanup(&info);
        return None;
    }

    let (width, height) = (bitmap.bmWidth as u32, bitmap.bmHeight as u32);
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // negative: top-down rows, so no flip afterwards
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let dc: HDC = CreateCompatibleDC(None);
    let scanned = GetDIBits(
        dc,
        info.hbmColor,
        0,
        height,
        Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
        &mut header,
        DIB_RGB_COLORS,
    );
    let _ = DeleteDC(dc);
    cleanup(&info);

    if scanned == 0 {
        return None;
    }

    // GDI hands back BGRA; iced wants RGBA.
    let mut opaque = true;
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        if px[3] != 0 {
            opaque = false;
        }
    }
    // A 24-bit icon leaves the alpha plane zeroed, which would render as nothing at all.
    if opaque {
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }

    iced::window::icon::from_rgba(pixels, width, height).ok()
}
