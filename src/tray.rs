//! The notification-area (tray) icon: a way to reach settings and exit even
//! when nothing is playing and the widget is hidden.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetSystemMetrics, LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXSMICON, SM_CYSMICON,
    WM_APP,
};

/// Sent to the owner window; lParam is the mouse message (WM_LBUTTONUP...).
pub const WM_TRAY: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    }
}

fn set_tip(nid: &mut NOTIFYICONDATAW, tip: &str) {
    let wide: Vec<u16> = tip.encode_utf16().take(nid.szTip.len() - 1).collect();
    nid.szTip = [0; 128];
    nid.szTip[..wide.len()].copy_from_slice(&wide);
}

/// The app icon embedded in the exe (resource 1), at tray size.
fn icon(module: HINSTANCE) -> HICON {
    unsafe {
        LoadImageW(
            Some(module),
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}

/// Returns false if Windows did not take the icon (Explorer is still
/// starting at sign-in, say); the caller tries again later.
pub fn add(hwnd: HWND, module: HINSTANCE, tip: &str) -> bool {
    let mut nid = data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon(module);
    set_tip(&mut nid, tip);
    unsafe {
        // An icon left from before (Explorer restarted) would make the add
        // fail; modifying it instead is just as good.
        let ok = Shell_NotifyIconW(NIM_ADD, &nid).as_bool() || Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool();
        // The shell keeps its own copy of the icon.
        if !nid.hIcon.is_invalid() {
            let _ = DestroyIcon(nid.hIcon);
        }
        ok
    }
}

pub fn update_tip(hwnd: HWND, tip: &str) {
    let mut nid = data(hwnd);
    nid.uFlags = NIF_TIP;
    set_tip(&mut nid, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
    }
}
