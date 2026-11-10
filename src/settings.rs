//! The settings window. Every change applies immediately and is saved.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, EnumDisplayMonitors, GetMonitorInfoW, GetSysColorBrush, MonitorFromPoint,
    COLOR_3DFACE, HDC, HFONT, HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, Config, DISPLAY_ALL, DISPLAY_MAIN, DISPLAY_ONE};

const ID_AUTOSTART: i32 = 100;
const ID_HIDE_PAUSED: i32 = 101;
const ID_SHOW_TIMES: i32 = 102;
const ID_TRAY: i32 = 103;
const ID_VOLUME: i32 = 104;
const ID_AVOID: i32 = 105;
const ID_SCROLL: i32 = 106;
const ID_SOURCE: i32 = 110;
const ID_DISPLAY: i32 = 111;
const ID_SIZE: i32 = 112;
const ID_WIDTH: i32 = 113;
const ID_RENDER: i32 = 114;
const ID_COLOR: i32 = 115;
const ID_RESET_POS: i32 = 120;
const ID_CLOSE: i32 = 121;
const ID_EXIT: i32 = 122;

// Control messages, styles and notifications.
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const BN_CLICKED: u32 = 0;
const CBN_SELCHANGE: u32 = 1;

/// Client height at 96 dpi; must match the layout in `open`.
const CLIENT_H: i32 = 650;

static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    /// Device names behind the per-display entries of the "Show on" list.
    static DISPLAYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The open settings window, or a null handle.
pub fn hwnd() -> HWND {
    HWND(SETTINGS_HWND.load(Ordering::Relaxed) as *mut c_void)
}

/// Register the standard control classes. Themed (visual styles) controls
/// come from the common-controls v6 dependency in the exe's manifest.
pub fn init_controls() {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
}

pub fn register_class(module: HINSTANCE) {
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: module,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: GetSysColorBrush(COLOR_3DFACE),
            hIcon: LoadIconW(Some(module), PCWSTR(1 as *const u16)).unwrap_or_default(),
            lpszClassName: w!("TaskbarMediaSettings"),
            ..Default::default()
        };
        RegisterClassW(&class);
    }
}

struct MonitorEntry {
    device: String,
    width: i32,
    height: i32,
    primary: bool,
}

unsafe extern "system" fn monitor_proc(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<MonitorEntry>);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(mon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
        let n = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        let r = info.monitorInfo.rcMonitor;
        list.push(MonitorEntry {
            device: String::from_utf16_lossy(&info.szDevice[..n]),
            width: r.right - r.left,
            height: r.bottom - r.top,
            primary: info.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
        });
    }
    true.into()
}

fn monitors() -> Vec<MonitorEntry> {
    let mut list: Vec<MonitorEntry> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_proc), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by(|a, b| a.device.cmp(&b.device));
    list
}

struct Builder {
    parent: HWND,
    module: HINSTANCE,
    font: HFONT,
    bold: HFONT,
    /// Pixels per 96-dpi unit.
    d: f32,
}

impl Builder {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.d).round() as i32
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn control(
        &self,
        class: PCWSTR,
        text: &str,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        font: HFONT,
    ) -> HWND {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
            self.px(x),
            self.px(y),
            self.px(w),
            self.px(h),
            Some(self.parent),
            Some(HMENU(id as isize as *mut c_void)),
            Some(self.module),
            None,
        )
        .unwrap_or_default();
        SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        hwnd
    }

    unsafe fn header(&self, text: &str, y: i32) {
        self.control(w!("STATIC"), text, 0, 16, y, 340, 20, -1, self.bold);
    }

    unsafe fn label(&self, text: &str, y: i32) {
        self.control(w!("STATIC"), text, 0, 16, y + 4, 126, 20, -1, self.font);
    }

    unsafe fn checkbox(&self, text: &str, y: i32, id: i32, checked: bool) {
        let h = self.control(
            w!("BUTTON"),
            text,
            BS_AUTOCHECKBOX | WS_TABSTOP.0,
            16,
            y,
            340,
            22,
            id,
            self.font,
        );
        SendMessageW(h, BM_SETCHECK, Some(WPARAM(checked as usize)), Some(LPARAM(0)));
    }

    unsafe fn combo(&self, y: i32, id: i32, items: &[String], selected: usize) {
        let h = self.control(
            w!("COMBOBOX"),
            "",
            CBS_DROPDOWNLIST | WS_VSCROLL.0 | WS_TABSTOP.0,
            146,
            y,
            218,
            260,
            id,
            self.font,
        );
        for item in items {
            let text = HSTRING::from(item.as_str());
            SendMessageW(h, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(text.as_ptr() as isize)));
        }
        SendMessageW(h, CB_SETCURSEL, Some(WPARAM(selected.min(items.len().saturating_sub(1)))), Some(LPARAM(0)));
    }

    unsafe fn button(&self, text: &str, x: i32, y: i32, w: i32, id: i32, default: bool) {
        let style = WS_TABSTOP.0 | if default { BS_DEFPUSHBUTTON } else { 0 };
        self.control(w!("BUTTON"), text, style, x, y, w, 28, id, self.font);
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Open the settings window, or bring it forward if it is already open.
pub fn open(module: HINSTANCE) {
    unsafe {
        let existing = hwnd();
        if !existing.is_invalid() && IsWindow(Some(existing)).as_bool() {
            let _ = ShowWindow(existing, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(existing);
            return;
        }

        let cfg = crate::current_config();
        let d = GetDpiForSystem().max(96) as f32 / 96.0;
        let (client_w, client_h) = ((380.0 * d) as i32, (CLIENT_H as f32 * d) as i32);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut wr = RECT { left: 0, top: 0, right: client_w, bottom: client_h };
        let _ = AdjustWindowRectEx(&mut wr, style, false, WINDOW_EX_STYLE(0));
        let (win_w, win_h) = (wr.right - wr.left, wr.bottom - wr.top);

        // Centre on the monitor the mouse is on (where the gear was clicked).
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let work = mi.rcWork;
        let x = work.left + ((work.right - work.left) - win_w) / 2;
        let y = work.top + ((work.bottom - work.top) - win_h) / 2;

        let Ok(parent) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("TaskbarMediaSettings"),
            w!("Taskbar Media settings"),
            style,
            x,
            y,
            win_w,
            win_h,
            None,
            None,
            Some(module),
            None,
        ) else {
            return;
        };
        SETTINGS_HWND.store(parent.0 as isize, Ordering::Relaxed);

        let make_font = |weight: i32| {
            CreateFontW(
                -(12.0 * d).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                0,
                w!("Segoe UI"),
            )
        };
        let b = Builder { parent, module, font: make_font(400), bold: make_font(600), d };

        // ---- General ----
        let mut y = 14;
        b.header("General", y);
        y += 26;
        b.checkbox("Start with Windows", y, ID_AUTOSTART, config::autostart_enabled());
        y += 26;
        b.checkbox("Hide the widget while playback is paused", y, ID_HIDE_PAUSED, cfg.hide_paused);
        y += 26;
        b.checkbox("Show an icon in the notification area", y, ID_TRAY, cfg.tray_icon);
        y += 26;
        b.checkbox("Scroll over the widget to change the volume", y, ID_VOLUME, cfg.wheel_volume);
        y += 30;
        b.label("Show media from", y);
        b.combo(y, ID_SOURCE, &strings(&["Any app", "Spotify only"]), cfg.spotify_only as usize);

        // ---- Taskbars ----
        y += 44;
        b.header("Taskbars", y);
        y += 26;
        let list = monitors();
        let mut items = strings(&["Main taskbar only", "All taskbars"]);
        let mut selected = match cfg.display_mode {
            DISPLAY_ALL => 1,
            _ => 0,
        };
        for (i, m) in list.iter().enumerate() {
            let number = m.device.trim_start_matches("\\\\.\\DISPLAY");
            let name = if number.chars().all(|c| c.is_ascii_digit()) && !number.is_empty() {
                format!("Display {number}")
            } else {
                m.device.clone()
            };
            let main = if m.primary { ", main" } else { "" };
            items.push(format!("{name} only ({} x {}{main})", m.width, m.height));
            if cfg.display_mode == DISPLAY_ONE && cfg.display_name == m.device {
                selected = i + 2;
            }
        }
        DISPLAYS.with(|v| *v.borrow_mut() = list.into_iter().map(|m| m.device).collect());
        b.label("Show on", y);
        b.combo(y, ID_DISPLAY, &items, selected);
        y += 32;
        b.control(
            w!("STATIC"),
            "Other displays need \"Show my taskbar on all displays\" turned on in Windows \
             taskbar settings. To nudge a widget sideways, hold Shift and scroll the mouse \
             wheel over it.",
            0,
            16,
            y,
            348,
            66,
            -1,
            b.font,
        );
        y += 70;
        b.checkbox("Keep the widget clear of app icons", y, ID_AVOID, cfg.avoid_apps);
        y += 30;
        b.button("Reset positions", 16, y, 130, ID_RESET_POS, false);

        // ---- Appearance ----
        y += 44;
        b.header("Appearance", y);
        y += 26;
        b.label("Text size", y);
        b.combo(y, ID_SIZE, &strings(&["Small", "Medium", "Large"]), cfg.text_size as usize);
        y += 30;
        b.label("Width", y);
        b.combo(y, ID_WIDTH, &strings(&["Compact", "Normal", "Wide"]), cfg.width as usize);
        y += 30;
        b.label("Rendering", y);
        b.combo(
            y,
            ID_RENDER,
            &strings(&["Smooth (high quality)", "Sharp (pixel-snapped)"]),
            (!cfg.smooth) as usize,
        );
        y += 30;
        b.label("Text color", y);
        b.combo(
            y,
            ID_COLOR,
            &strings(&["Automatic (match Windows)", "Light text", "Dark text"]),
            cfg.text_color as usize,
        );
        y += 34;
        b.checkbox("Show elapsed and total time", y, ID_SHOW_TIMES, cfg.show_times);
        y += 26;
        b.checkbox("Scroll titles that are too long to fit", y, ID_SCROLL, cfg.scroll_titles);

        // ---- Buttons ----
        y += 38;
        b.button("Exit Taskbar Media", 16, y, 150, ID_EXIT, false);
        b.button("Close", 274, y, 90, ID_CLOSE, true);
        debug_assert_eq!(y + 28 + 14, CLIENT_H);

        let _ = ShowWindow(parent, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(parent);
    }
}

unsafe fn checked(parent: HWND, id: i32) -> bool {
    match GetDlgItem(Some(parent), id) {
        Ok(h) => SendMessageW(h, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1,
        Err(_) => false,
    }
}

unsafe fn selection(parent: HWND, id: i32) -> usize {
    match GetDlgItem(Some(parent), id) {
        Ok(h) => SendMessageW(h, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0.max(0) as usize,
        Err(_) => 0,
    }
}

unsafe fn on_command(hwnd: HWND, id: i32, code: u32) {
    match (id, code) {
        (ID_AUTOSTART, BN_CLICKED) => config::set_autostart(checked(hwnd, id)),
        (ID_HIDE_PAUSED, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c: &mut Config| c.hide_paused = v);
        }
        (ID_TRAY, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.tray_icon = v);
        }
        (ID_VOLUME, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.wheel_volume = v);
        }
        (ID_AVOID, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.avoid_apps = v);
        }
        (ID_SCROLL, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.scroll_titles = v);
        }
        (ID_SHOW_TIMES, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.show_times = v);
        }
        (ID_SOURCE, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) == 1;
            crate::update_config(|c| c.spotify_only = v);
        }
        (ID_DISPLAY, CBN_SELCHANGE) => {
            let sel = selection(hwnd, id);
            let device = if sel >= 2 {
                DISPLAYS.with(|v| v.borrow().get(sel - 2).cloned())
            } else {
                None
            };
            crate::update_config(|c| match (sel, device) {
                (1, _) => c.display_mode = DISPLAY_ALL,
                (_, Some(name)) => {
                    c.display_mode = DISPLAY_ONE;
                    c.display_name = name;
                }
                _ => c.display_mode = DISPLAY_MAIN,
            });
        }
        (ID_SIZE, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) as u32;
            crate::update_config(|c| c.text_size = v.min(2));
        }
        (ID_WIDTH, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) as u32;
            crate::update_config(|c| c.width = v.min(2));
        }
        (ID_RENDER, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) == 0;
            crate::update_config(|c| c.smooth = v);
        }
        (ID_COLOR, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) as u32;
            crate::update_config(|c| c.text_color = v.min(2));
        }
        (ID_RESET_POS, BN_CLICKED) => crate::update_config(|c| {
            c.offset = 0;
            c.offset2 = 0;
        }),
        (ID_CLOSE, BN_CLICKED) => {
            let _ = DestroyWindow(hwnd);
        }
        (ID_EXIT, BN_CLICKED) => PostQuitMessage(0),
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            // Enter / Esc arrive as IDOK / IDCANCEL through IsDialogMessage.
            if id == 1 || id == 2 {
                let _ = DestroyWindow(hwnd);
            } else {
                on_command(hwnd, id, code);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            SETTINGS_HWND.store(0, Ordering::Relaxed);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
