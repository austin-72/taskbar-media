//! The settings window: four tabs of options. Every change applies
//! immediately and is saved.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{w, BOOL, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, EnumDisplayMonitors, GetMonitorInfoW, GetSysColorBrush, MonitorFromPoint,
    COLOR_3DFACE, HDC, HFONT, HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, NMHDR,
    TCIF_TEXT, TCITEMW,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, Config, DISPLAY_ALL, DISPLAY_MAIN, DISPLAY_ONE};

// Checkboxes.
const ID_AUTOSTART: i32 = 100;
const ID_HIDE_PAUSED: i32 = 101;
const ID_SHOW_TIMES: i32 = 102;
const ID_TRAY: i32 = 103;
const ID_UPDATES: i32 = 104;
const ID_AVOID: i32 = 105;
const ID_SCROLL: i32 = 106;
// Drop-down lists.
const ID_SOURCE: i32 = 110;
const ID_DISPLAY: i32 = 111;
const ID_SIZE: i32 = 112;
const ID_WIDTH: i32 = 113;
const ID_RENDER: i32 = 114;
const ID_COLOR: i32 = 115;
const ID_WHEEL: i32 = 116;
const ID_BUTTON_SIZE: i32 = 117;
const ID_TEXT_LAYOUT: i32 = 118;
const ID_FONT: i32 = 119;
const ID_ACCENT: i32 = 130;
const ID_ART_SHAPE: i32 = 131;
const ID_SEEK_SIZE: i32 = 132;
const ID_BACKGROUND: i32 = 133;
const ID_ART_CLICK: i32 = 134;
const ID_TITLE_CLICK: i32 = 135;
const ID_OUTPUT_CLICK: i32 = 136;
const ID_OUTPUT_A: i32 = 137;
const ID_OUTPUT_B: i32 = 138;
// Buttons.
const ID_RESET_POS: i32 = 120;
const ID_CLOSE: i32 = 121;
const ID_EXIT: i32 = 122;
const ID_RELEASES: i32 = 123;
// The layout editor.
const ID_ITEMS: i32 = 140;
const ID_ITEM_UP: i32 = 141;
const ID_ITEM_DOWN: i32 = 142;
const ID_ITEM_TOGGLE: i32 = 143;
const ID_ITEM_RESET: i32 = 144;
const ID_TABS: i32 = 150;

// Control messages, styles and notifications.
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCURSEL: u32 = 0x0188;
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_INSERTITEMW: u32 = 0x133E;
const TCN_SELCHANGE: u32 = -551i32 as u32;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const BN_CLICKED: u32 = 0;
const CBN_SELCHANGE: u32 = 1;
const LBN_SELCHANGE: u32 = 1;

// Window layout at 96 dpi: a tab strip, the tab's controls, a row of buttons.
const CLIENT_W: i32 = 400;
const TOP: i32 = 48;
const BUTTONS_Y: i32 = 482;
const CLIENT_H: i32 = BUTTONS_Y + 28 + 14;
const TAB_NAMES: [&str; 4] = ["General", "Layout", "Appearance", "Behavior"];

/// Controls created with this tab index are always visible.
const NO_TAB: usize = usize::MAX;

static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    /// Device names behind the per-display entries of the "Show on" list.
    static DISPLAYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Device ids behind the rows of the two output-device lists (row 0 is
    /// "not set").
    static OUTPUTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Every tab's controls, as (tab index, window handle).
    static PAGES: RefCell<Vec<(usize, isize)>> = const { RefCell::new(Vec::new()) };
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
            dwICC: ICC_STANDARD_CLASSES | ICC_TAB_CLASSES,
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
    /// The tab the controls being created belong to.
    tab: Cell<usize>,
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
        // Only the first tab's controls start out visible.
        let tab = self.tab.get();
        let visible = if tab == NO_TAB || tab == 0 { WS_VISIBLE } else { WINDOW_STYLE(0) };
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(text),
            WS_CHILD | visible | WINDOW_STYLE(style),
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
        if tab != NO_TAB {
            PAGES.with(|p| p.borrow_mut().push((tab, hwnd.0 as isize)));
        }
        hwnd
    }

    unsafe fn header(&self, text: &str, y: i32) {
        self.control(w!("STATIC"), text, 0, 16, y, 360, 20, -1, self.bold);
    }

    unsafe fn text(&self, text: &str, y: i32, h: i32) {
        self.control(w!("STATIC"), text, 0, 16, y, 368, h, -1, self.font);
    }

    unsafe fn checkbox(&self, text: &str, y: i32, id: i32, checked: bool) {
        let h = self.control(
            w!("BUTTON"),
            text,
            BS_AUTOCHECKBOX | WS_TABSTOP.0,
            16,
            y,
            360,
            22,
            id,
            self.font,
        );
        SendMessageW(h, BM_SETCHECK, Some(WPARAM(checked as usize)), Some(LPARAM(0)));
    }

    /// A label with a drop-down list beside it.
    unsafe fn choice(&self, label: &str, y: i32, id: i32, items: &[&str], selected: u32) {
        let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        self.choice_owned(label, y, id, &items, selected as usize);
    }

    unsafe fn choice_owned(&self, label: &str, y: i32, id: i32, items: &[String], selected: usize) {
        self.control(w!("STATIC"), label, 0, 16, y + 4, 126, 20, -1, self.font);
        let h = self.control(
            w!("COMBOBOX"),
            "",
            CBS_DROPDOWNLIST | WS_VSCROLL.0 | WS_TABSTOP.0,
            146,
            y,
            238,
            300,
            id,
            self.font,
        );
        for item in items {
            let text = HSTRING::from(item.as_str());
            SendMessageW(h, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(text.as_ptr() as isize)));
        }
        let selected = selected.min(items.len().saturating_sub(1));
        SendMessageW(h, CB_SETCURSEL, Some(WPARAM(selected)), Some(LPARAM(0)));
    }

    unsafe fn button(&self, text: &str, x: i32, y: i32, w: i32, id: i32, default: bool) {
        let style = WS_TABSTOP.0 | if default { BS_DEFPUSHBUTTON } else { 0 };
        self.control(w!("BUTTON"), text, style, x, y, w, 28, id, self.font);
    }
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
        let (client_w, client_h) = ((CLIENT_W as f32 * d) as i32, (CLIENT_H as f32 * d) as i32);
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
        PAGES.with(|p| p.borrow_mut().clear());

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
        let b = Builder {
            parent,
            module,
            font: make_font(400),
            bold: make_font(600),
            d,
            tab: Cell::new(NO_TAB),
        };

        // The tab strip. Only the strip: the controls of each tab sit
        // directly on the window and are shown or hidden as tabs change.
        let tabs = b.control(
            w!("SysTabControl32"),
            "",
            WS_TABSTOP.0 | WS_CLIPSIBLINGS.0,
            8,
            8,
            CLIENT_W - 16,
            26,
            ID_TABS,
            b.font,
        );
        for (i, name) in TAB_NAMES.iter().enumerate() {
            let mut text: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let item = TCITEMW { mask: TCIF_TEXT, pszText: PWSTR(text.as_mut_ptr()), ..Default::default() };
            SendMessageW(tabs, TCM_INSERTITEMW, Some(WPARAM(i)), Some(LPARAM(&item as *const _ as isize)));
        }

        // ---- General ----------------------------------------------------
        b.tab.set(0);
        let mut y = TOP;
        b.checkbox("Start with Windows", y, ID_AUTOSTART, config::autostart_enabled());
        y += 26;
        b.checkbox("Hide the widget while playback is paused", y, ID_HIDE_PAUSED, cfg.hide_paused);
        y += 26;
        b.checkbox("Show an icon in the notification area", y, ID_TRAY, cfg.tray_icon);
        y += 26;
        b.checkbox("Check GitHub for new versions", y, ID_UPDATES, cfg.check_updates);
        y += 30;
        b.choice("Show media from", y, ID_SOURCE, &["Any app", "Spotify only"], cfg.spotify_only as u32);

        y += 44;
        b.header("Taskbars", y);
        y += 26;
        let list = monitors();
        let mut items: Vec<String> = vec!["Main taskbar only".into(), "All taskbars".into()];
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
        b.choice_owned("Show on", y, ID_DISPLAY, &items, selected);
        y += 32;
        b.text(
            "Other displays need \"Show my taskbar on all displays\" turned on in Windows \
             taskbar settings. To nudge a widget sideways, hold Shift and scroll the mouse \
             wheel over it.",
            y,
            52,
        );
        y += 58;
        b.checkbox("Keep the widget clear of app icons", y, ID_AVOID, cfg.avoid_apps);
        y += 30;
        b.button("Reset positions", 16, y, 130, ID_RESET_POS, false);

        y += 46;
        b.header("About", y);
        y += 24;
        let about = match crate::update::available() {
            Some(newer) => format!(
                "Taskbar Media {}. Version {newer} is available.",
                crate::update::VERSION
            ),
            None => format!("Taskbar Media {}", crate::update::VERSION),
        };
        b.text(&about, y, 20);
        y += 24;
        b.button("Open the releases page", 16, y, 180, ID_RELEASES, false);
        debug_assert!(y + 28 <= BUTTONS_Y - 10);

        // ---- Layout -----------------------------------------------------
        b.tab.set(1);
        let mut y = TOP;
        b.text("Items on the widget, in order from left to right:", y, 20);
        y += 24;
        b.control(
            w!("LISTBOX"),
            "",
            LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | WS_BORDER.0 | WS_VSCROLL.0 | WS_TABSTOP.0,
            16,
            y,
            214,
            166,
            ID_ITEMS,
            b.font,
        );
        b.button("Move up", 242, y, 142, ID_ITEM_UP, false);
        b.button("Move down", 242, y + 32, 142, ID_ITEM_DOWN, false);
        b.button("Show / hide", 242, y + 72, 142, ID_ITEM_TOGGLE, false);
        b.button("Reset layout", 242, y + 138, 142, ID_ITEM_RESET, false);
        y += 174;
        b.text(
            "Shuffle and repeat only appear with players that support them. With the \
             settings button hidden, right-click the widget to get back here.",
            y,
            52,
        );
        y += 60;
        b.choice("Text width", y, ID_WIDTH, &["Compact", "Normal", "Wide"], cfg.width);
        y += 30;
        b.choice("Button size", y, ID_BUTTON_SIZE, &["Small", "Medium", "Large"], cfg.button_size);
        y += 30;
        b.choice(
            "Text",
            y,
            ID_TEXT_LAYOUT,
            &[
                "Title \u{00B7} Artist, seek bar below",
                "Title, seek bar below",
                "Artist \u{00B7} Title, seek bar below",
                "Title, artist below",
                "Title, album below",
            ],
            cfg.text_layout,
        );

        // ---- Appearance -------------------------------------------------
        b.tab.set(2);
        let mut y = TOP;
        b.choice("Text size", y, ID_SIZE, &["Small", "Medium", "Large"], cfg.text_size);
        y += 30;
        b.choice("Font", y, ID_FONT, &config::FONTS, cfg.font);
        y += 30;
        b.choice(
            "Text color",
            y,
            ID_COLOR,
            &["Automatic (match Windows)", "Light text", "Dark text"],
            cfg.text_color,
        );
        y += 30;
        b.choice(
            "Rendering",
            y,
            ID_RENDER,
            &["Smooth (high quality)", "Sharp (pixel-snapped)"],
            (!cfg.smooth) as u32,
        );
        y += 30;
        b.choice(
            "Highlight color",
            y,
            ID_ACCENT,
            &["From the album art", "Windows accent color", "Same as the text"],
            cfg.accent_mode,
        );
        y += 30;
        b.choice("Album art shape", y, ID_ART_SHAPE, &["Rounded", "Square", "Circle"], cfg.art_shape);
        y += 30;
        b.choice("Seek bar", y, ID_SEEK_SIZE, &["Thin", "Normal", "Thick"], cfg.seek_size);
        y += 30;
        b.choice("Background", y, ID_BACKGROUND, &["None", "Subtle", "Solid"], cfg.background);
        y += 36;
        b.checkbox("Show elapsed and total time", y, ID_SHOW_TIMES, cfg.show_times);
        y += 26;
        b.checkbox("Scroll titles that are too long to fit", y, ID_SCROLL, cfg.scroll_titles);

        // ---- Behavior ---------------------------------------------------
        b.tab.set(3);
        let mut y = TOP;
        b.choice(
            "Mouse wheel",
            y,
            ID_WHEEL,
            &["Does nothing", "System volume", "Player volume"],
            cfg.wheel_mode,
        );
        y += 30;
        b.choice(
            "Click album art",
            y,
            ID_ART_CLICK,
            &["Open the player", "Play / pause", "Does nothing"],
            cfg.art_click,
        );
        y += 30;
        b.choice(
            "Click the title",
            y,
            ID_TITLE_CLICK,
            &["Copy \"Artist - Title\"", "Open the player", "Does nothing"],
            cfg.title_click,
        );

        y += 44;
        b.header("Output device button", y);
        y += 24;
        b.text(
            "Turn the button on in the Layout tab. Right-clicking it always lists the              devices.",
            y,
            36,
        );
        y += 42;
        b.choice(
            "Click the button",
            y,
            ID_OUTPUT_CLICK,
            &["List the devices", "Switch between two devices"],
            cfg.output_click,
        );
        let outputs = crate::devices::list();
        let mut names: Vec<String> = vec!["(not set)".into()];
        names.extend(outputs.iter().map(|d| d.name.clone()));
        let row_of = |id: &str| outputs.iter().position(|d| d.id == id).map_or(0, |i| i + 1);
        y += 30;
        b.choice_owned("First device", y, ID_OUTPUT_A, &names, row_of(&cfg.output_a));
        y += 30;
        b.choice_owned("Second device", y, ID_OUTPUT_B, &names, row_of(&cfg.output_b));
        OUTPUTS.with(|v| *v.borrow_mut() = outputs.into_iter().map(|d| d.id).collect());

        // ---- Buttons ----------------------------------------------------
        b.tab.set(NO_TAB);
        b.button("Exit Taskbar Media", 16, BUTTONS_Y, 150, ID_EXIT, false);
        b.button("Close", CLIENT_W - 16 - 90, BUTTONS_Y, 90, ID_CLOSE, true);

        fill_items(parent, &cfg, 0);
        let _ = ShowWindow(parent, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(parent);
    }
}

/// Show the controls of one tab and hide the rest.
unsafe fn show_tab(tab: usize) {
    PAGES.with(|pages| {
        for (page, raw) in pages.borrow().iter() {
            let show = if *page == tab { SW_SHOW } else { SW_HIDE };
            let _ = ShowWindow(HWND(*raw as *mut c_void), show);
        }
    });
}

/// Fill the layout editor's list from the settings and select row `select`.
unsafe fn fill_items(parent: HWND, cfg: &Config, select: usize) {
    let Ok(list) = GetDlgItem(Some(parent), ID_ITEMS) else { return };
    SendMessageW(list, LB_RESETCONTENT, Some(WPARAM(0)), Some(LPARAM(0)));
    for (item, shown) in &cfg.layout {
        let label = config::item_label(*item);
        let text = HSTRING::from(if *shown { label.to_string() } else { format!("{label}  (hidden)") });
        SendMessageW(list, LB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(text.as_ptr() as isize)));
    }
    let select = select.min(cfg.layout.len().saturating_sub(1));
    SendMessageW(list, LB_SETCURSEL, Some(WPARAM(select)), Some(LPARAM(0)));
}

unsafe fn checked(parent: HWND, id: i32) -> bool {
    match GetDlgItem(Some(parent), id) {
        Ok(h) => SendMessageW(h, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1,
        Err(_) => false,
    }
}

/// The selected row of a drop-down list.
unsafe fn selection(parent: HWND, id: i32) -> usize {
    match GetDlgItem(Some(parent), id) {
        Ok(h) => SendMessageW(h, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0.max(0) as usize,
        Err(_) => 0,
    }
}

/// Change the layout with `edit(layout, selected row)`, which returns the row
/// to select afterwards, then save, apply and refresh the list.
unsafe fn edit_layout(parent: HWND, edit: impl FnOnce(&mut Vec<(u8, bool)>, usize) -> usize) {
    let Ok(list) = GetDlgItem(Some(parent), ID_ITEMS) else { return };
    let row = SendMessageW(list, LB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0;
    let mut layout = crate::current_config().layout;
    if row < 0 || row as usize >= layout.len() {
        return;
    }
    let select = edit(&mut layout, row as usize);
    crate::update_config(|c| c.layout = layout);
    fill_items(parent, &crate::current_config(), select);
}

unsafe fn on_command(hwnd: HWND, id: i32, code: u32) {
    // Drop-down lists that simply store the selected row.
    if code == CBN_SELCHANGE {
        let v = selection(hwnd, id) as u32;
        let field: Option<fn(&mut Config) -> &mut u32> = match id {
            ID_SIZE => Some(|c| &mut c.text_size),
            ID_WIDTH => Some(|c| &mut c.width),
            ID_COLOR => Some(|c| &mut c.text_color),
            ID_WHEEL => Some(|c| &mut c.wheel_mode),
            ID_BUTTON_SIZE => Some(|c| &mut c.button_size),
            ID_TEXT_LAYOUT => Some(|c| &mut c.text_layout),
            ID_FONT => Some(|c| &mut c.font),
            ID_ACCENT => Some(|c| &mut c.accent_mode),
            ID_ART_SHAPE => Some(|c| &mut c.art_shape),
            ID_SEEK_SIZE => Some(|c| &mut c.seek_size),
            ID_BACKGROUND => Some(|c| &mut c.background),
            ID_ART_CLICK => Some(|c| &mut c.art_click),
            ID_TITLE_CLICK => Some(|c| &mut c.title_click),
            ID_OUTPUT_CLICK => Some(|c| &mut c.output_click),
            _ => None,
        };
        if let Some(field) = field {
            crate::update_config(|c| *field(c) = v);
            return;
        }
    }

    match (id, code) {
        (ID_AUTOSTART, BN_CLICKED) => config::set_autostart(checked(hwnd, id)),
        (ID_HIDE_PAUSED, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.hide_paused = v);
        }
        (ID_TRAY, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.tray_icon = v);
        }
        (ID_UPDATES, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.check_updates = v);
        }
        (ID_AVOID, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.avoid_apps = v);
        }
        (ID_SHOW_TIMES, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.show_times = v);
        }
        (ID_SCROLL, BN_CLICKED) => {
            let v = checked(hwnd, id);
            crate::update_config(|c| c.scroll_titles = v);
        }
        (ID_SOURCE, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) == 1;
            crate::update_config(|c| c.spotify_only = v);
        }
        (ID_RENDER, CBN_SELCHANGE) => {
            let v = selection(hwnd, id) == 0;
            crate::update_config(|c| c.smooth = v);
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
        (ID_OUTPUT_A | ID_OUTPUT_B, CBN_SELCHANGE) => {
            // Row 0 is "(not set)".
            let row = selection(hwnd, id);
            let device = if row == 0 {
                String::new()
            } else {
                OUTPUTS.with(|v| v.borrow().get(row - 1).cloned()).unwrap_or_default()
            };
            crate::update_config(|c| {
                if id == ID_OUTPUT_A {
                    c.output_a = device;
                } else {
                    c.output_b = device;
                }
            });
        }
        (ID_ITEM_UP, BN_CLICKED) => edit_layout(hwnd, |layout, row| {
            if row == 0 {
                return row;
            }
            layout.swap(row, row - 1);
            row - 1
        }),
        (ID_ITEM_DOWN, BN_CLICKED) => edit_layout(hwnd, |layout, row| {
            if row + 1 >= layout.len() {
                return row;
            }
            layout.swap(row, row + 1);
            row + 1
        }),
        (ID_ITEM_TOGGLE, BN_CLICKED) => edit_layout(hwnd, |layout, row| {
            layout[row].1 = !layout[row].1;
            row
        }),
        (ID_ITEM_RESET, BN_CLICKED) => edit_layout(hwnd, |layout, _| {
            *layout = config::default_layout(true);
            0
        }),
        (ID_ITEMS, LBN_SELCHANGE) => {}
        (ID_RESET_POS, BN_CLICKED) => crate::update_config(|c| {
            c.offset = 0;
            c.offset2 = 0;
        }),
        (ID_RELEASES, BN_CLICKED) => crate::open_releases_page(),
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
        WM_NOTIFY => {
            let header = &*(lparam.0 as *const NMHDR);
            if header.idFrom == ID_TABS as usize && header.code == TCN_SELCHANGE {
                let tab = SendMessageW(header.hwndFrom, TCM_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0;
                show_tab(tab.max(0) as usize);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            SETTINGS_HWND.store(0, Ordering::Relaxed);
            PAGES.with(|p| p.borrow_mut().clear());
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
