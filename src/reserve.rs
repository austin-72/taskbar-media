//! Windows 10 only: make room for the widget on the main taskbar.
//!
//! The Windows 10 taskbar lays its app buttons out inside an ordinary child
//! window (`MSTaskSwWClass`, inside `ReBarWindow32`). Narrowing that window
//! makes Explorer fit the buttons into less space, which leaves a real gap
//! for the widget instead of the widget sitting on top of buttons. Explorer
//! puts the width back whenever it re-lays out the taskbar, so this is
//! re-applied on every tick, and undone when the widget hides or exits.
//!
//! Windows 11 draws its taskbar differently and offers nothing equivalent;
//! there the widget shrinks instead (see `probe`).

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowRect, SetWindowPos, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SWP_NOZORDER,
};

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
}

/// True on Windows 10 (and earlier); false on Windows 11 and later.
pub fn supported() -> bool {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    unsafe { RtlGetVersion(&mut info) == 0 && info.dwBuildNumber != 0 && info.dwBuildNumber < 22000 }
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The right edge (screen x) the button area was last narrowed to.
static TARGET: AtomicI32 = AtomicI32::new(0);

struct Parts {
    rebar: HWND,
    tasks: HWND,
    rebar_rect: RECT,
    tasks_rect: RECT,
}

unsafe fn parts(taskbar: HWND) -> Option<Parts> {
    let rebar = FindWindowExW(Some(taskbar), None, w!("ReBarWindow32"), None).ok()?;
    let tasks = FindWindowExW(Some(rebar), None, w!("MSTaskSwWClass"), None).ok()?;
    let mut rebar_rect = RECT::default();
    let mut tasks_rect = RECT::default();
    GetWindowRect(rebar, &mut rebar_rect).ok()?;
    GetWindowRect(tasks, &mut tasks_rect).ok()?;
    Some(Parts { rebar, tasks, rebar_rect, tasks_rect })
}

unsafe fn set_right_edge(p: &Parts, right: i32) {
    let mut origin = POINT { x: p.tasks_rect.left, y: p.tasks_rect.top };
    if !ScreenToClient(p.rebar, &mut origin).as_bool() {
        return;
    }
    let _ = SetWindowPos(
        p.tasks,
        None,
        origin.x,
        origin.y,
        right - p.tasks_rect.left,
        p.tasks_rect.bottom - p.tasks_rect.top,
        SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
    );
}

/// Narrow the app-button area so that it ends at `right` (screen x), just
/// left of the widget. Returns false, changing nothing, if this taskbar is
/// not laid out the way this code expects.
pub fn apply(taskbar: HWND, right: i32) -> bool {
    unsafe {
        let Some(p) = parts(taskbar) else { return false };
        let full = p.rebar_rect.right;
        let current = p.tasks_rect.right;
        let ours = ACTIVE.load(Ordering::Relaxed) && (current - TARGET.load(Ordering::Relaxed)).abs() <= 1;

        // Only when the button area is the last thing before the tray. If
        // other toolbars sit to its right, leave everything alone.
        if !ours && (current - full).abs() > 2 {
            return false;
        }
        // Never take more than half of the button area.
        let least = p.tasks_rect.left + (full - p.tasks_rect.left) / 2;
        if right >= full || right < least {
            restore(taskbar);
            return false;
        }
        if (current - right).abs() > 1 {
            set_right_edge(&p, right);
        }
        TARGET.store(right, Ordering::Relaxed);
        ACTIVE.store(true, Ordering::Relaxed);
        true
    }
}

/// Give the app-button area its full width back.
pub fn restore(taskbar: HWND) {
    if !ACTIVE.swap(false, Ordering::Relaxed) {
        return;
    }
    unsafe {
        let Some(p) = parts(taskbar) else { return };
        // Only undo our own change; Explorer may already have reset it.
        if (p.tasks_rect.right - TARGET.load(Ordering::Relaxed)).abs() <= 1 {
            set_right_edge(&p, p.rebar_rect.right);
        }
    }
}
