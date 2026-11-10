//! Looks inside the Windows 11 taskbar with UI Automation to find where the
//! app buttons end and where the tray / clock begins. Windows 11 draws both
//! without child windows, so their positions cannot be read any other way.
//!
//! Runs on its own thread because these are slow cross-process calls. When
//! nothing recognisable is found (Windows 10, or a future taskbar redesign)
//! the result is simply empty and the widget falls back to its defaults.

use std::sync::Mutex;
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetWindowRect};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Probe {
    /// Left edge of the tray icons / clock, in screen pixels.
    pub tray_left: Option<i32>,
    /// Right edge of the last app button, in screen pixels.
    pub apps_right: Option<i32>,
}

static RESULTS: Mutex<Vec<(isize, Probe)>> = Mutex::new(Vec::new());

/// The latest measurements for a taskbar window.
pub fn get(taskbar: HWND) -> Probe {
    let key = taskbar.0 as isize;
    RESULTS
        .lock()
        .ok()
        .and_then(|r| r.iter().find(|(h, _)| *h == key).map(|(_, p)| *p))
        .unwrap_or_default()
}

pub fn spawn() {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        else {
            return;
        };
        loop {
            let mut results = Vec::new();
            for hwnd in taskbars() {
                results.push((hwnd.0 as isize, measure(&uia, hwnd).unwrap_or_default()));
            }
            if let Ok(mut guard) = RESULTS.lock() {
                *guard = results;
            }
            std::thread::sleep(Duration::from_millis(2000));
        }
    });
}

unsafe fn taskbars() -> Vec<HWND> {
    let mut list = Vec::new();
    if let Ok(main) = FindWindowW(w!("Shell_TrayWnd"), None) {
        list.push(main);
    }
    let mut prev: Option<HWND> = None;
    for _ in 0..16 {
        match FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), None) {
            Ok(h) if !h.is_invalid() => {
                list.push(h);
                prev = Some(h);
            }
            _ => break,
        }
    }
    list
}

unsafe fn measure(uia: &IUIAutomation, taskbar: HWND) -> windows::core::Result<Probe> {
    let mut bar = RECT::default();
    GetWindowRect(taskbar, &mut bar)?;
    let middle = (bar.left + bar.right) / 2;

    let root = uia.ElementFromHandle(taskbar)?;
    let cache = uia.CreateCacheRequest()?;
    cache.AddProperty(UIA_ClassNamePropertyId)?;
    cache.AddProperty(UIA_AutomationIdPropertyId)?;
    cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
    let all = root.FindAllBuildCache(TreeScope_Descendants, &uia.CreateTrueCondition()?, &cache)?;

    let mut probe = Probe::default();
    for i in 0..all.Length()? {
        let Ok(element) = all.GetElement(i) else { continue };
        let Ok(r) = element.CachedBoundingRectangle() else { continue };
        if r.right <= r.left || r.bottom <= r.top || r.right <= bar.left || r.left >= bar.right {
            continue;
        }
        let class = element.CachedClassName().map(|s| s.to_string()).unwrap_or_default();
        let id = element.CachedAutomationId().map(|s| s.to_string()).unwrap_or_default();

        // Only individual buttons count; containers that span the taskbar
        // carry similar class names and would give useless edges.
        let button_sized = r.right - r.left < (bar.right - bar.left) / 4;
        let is_tray = button_sized
            && (class.starts_with("SystemTray.") || id == "SystemTrayIcon" || id == "NotifyItemIcon");
        let is_app = button_sized
            && (class.starts_with("Taskbar.TaskListButton")
                || id.starts_with("Appid:")
                || matches!(id.as_str(), "StartButton" | "SearchButton" | "TaskViewButton"));

        if is_tray {
            // The tray is always in the right-hand half.
            if r.left > middle {
                probe.tray_left = Some(probe.tray_left.map_or(r.left, |v| v.min(r.left)));
            }
        } else if is_app {
            probe.apps_right = Some(probe.apps_right.map_or(r.right, |v| v.max(r.right)));
        }
    }
    Ok(probe)
}
