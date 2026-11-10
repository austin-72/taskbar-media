//! Taskbar Media: a now-playing widget that sits on the Windows taskbar, just
//! left of the notification area / clock.
//!
//! Album art (click to open the player), title and artist, a seek bar, and
//! previous / play-pause / next buttons. Works with anything that reports to
//! Windows' media controls: Spotify, browsers, Media Player, and so on.

#![windows_subsystem = "windows"]

mod launch;
mod media;
mod render;

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_SHIFT,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use media::{Cmd, Snapshot};
use render::{Frame, Hit, Layout, Renderer};

const WM_APP_UPDATE: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const TICK_MS: u32 = 200;
const WM_MOUSELEAVE: u32 = 0x02A3;

const SETTINGS_KEY: PCWSTR = w!("Software\\TaskbarMedia");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("TaskbarMedia");
const THEME_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

static WIDGET_HWND: AtomicIsize = AtomicIsize::new(0);
static QUITTING: AtomicBool = AtomicBool::new(false);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` against the app state. Returns None if the state is already in use
/// further up the stack (a nested message), in which case the message is
/// simply skipped; the next timer tick catches up.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(f)))
}

struct SeekOverride {
    position: i64,
    at: Instant,
    last_updated_then: i64,
}

struct App {
    hwnd: HWND,
    owner: HWND,
    shared: Arc<Mutex<Snapshot>>,
    tx: Sender<Cmd>,
    snap: Snapshot,
    renderer: Renderer,
    layout: Layout,
    last_frame: Option<Frame>,
    rect: (i32, i32, i32, i32),
    visible: bool,
    light: bool,
    ticks: u32,
    offset: i32,
    hover: Hit,
    pressed: Hit,
    dragging: bool,
    drag_fraction: f32,
    tracking_leave: bool,
    seek_override: Option<SeekOverride>,
}

struct Taskbar {
    hwnd: HWND,
    rect: RECT,
    tray_left: i32,
    dpi: u32,
}

fn locate_taskbar() -> Option<Taskbar> {
    unsafe {
        let hwnd = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;

        // The notification area (tray icons + clock). The widget goes
        // immediately to its left.
        let mut tray_left = rect.right - 240;
        if let Ok(tray) = FindWindowExW(Some(hwnd), None, w!("TrayNotifyWnd"), None) {
            let mut tr = RECT::default();
            if GetWindowRect(tray, &mut tr).is_ok() && tr.right > tr.left {
                tray_left = tr.left;
            }
        }
        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };
        Some(Taskbar { hwnd, rect, tray_left, dpi })
    }
}

/// True while a full-screen app (game, video, slideshow) is in front on the
/// taskbar's monitor, which is when the taskbar gets out of the way.
fn fullscreen_app_running(taskbar: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == taskbar {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return false;
        }
        let mut wr = RECT::default();
        if GetWindowRect(fg, &mut wr).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(taskbar, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        let covers = wr.left <= m.left && wr.top <= m.top && wr.right >= m.right && wr.bottom >= m.bottom;
        // A maximised window also covers the monitor when the taskbar is set
        // to auto-hide, so ask the shell whether it really is full-screen.
        covers
            && SHQueryUserNotificationState()
                .is_ok_and(|s| s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN)
    }
}

fn filetime_now() -> i64 {
    let ft = unsafe { GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn format_time(ticks: i64) -> String {
    let secs = (ticks.max(0) / 10_000_000) as u64;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn read_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

fn write_dword(key: PCWSTR, value: PCWSTR, data: u32) {
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            REG_DWORD.0,
            Some(&data as *const u32 as *const c_void),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn autostart_enabled() -> bool {
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size))
    };
    status == ERROR_SUCCESS
}

fn set_autostart(enable: bool) {
    unsafe {
        if !enable {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            return;
        }
        let mut path = [0u16; 1024];
        let len = GetModuleFileNameW(None, &mut path) as usize;
        if len == 0 || len >= path.len() {
            return;
        }
        let mut cmd: Vec<u16> = Vec::with_capacity(len + 3);
        cmd.push(b'"' as u16);
        cmd.extend_from_slice(&path[..len]);
        cmd.push(b'"' as u16);
        cmd.push(0);
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            REG_SZ.0,
            Some(cmd.as_ptr() as *const c_void),
            (cmd.len() * 2) as u32,
        );
    }
}

impl App {
    fn has_timeline(&self) -> bool {
        self.snap.end > self.snap.start
    }

    /// Players only report their position now and then, so advance it
    /// ourselves from the time of the last report.
    fn current_position(&self) -> i64 {
        let s = &self.snap;
        let mut pos = if let Some(o) = &self.seek_override {
            let mut p = o.position;
            if s.playing {
                p += (o.at.elapsed().as_secs_f64() * 1e7 * s.rate) as i64;
            }
            p
        } else {
            let mut p = s.position;
            if s.playing && s.last_updated > 0 {
                let delta = filetime_now() - s.last_updated;
                if delta > 0 && delta < 864_000_000_000 {
                    p += (delta as f64 * s.rate) as i64;
                }
            }
            p
        };
        if self.has_timeline() {
            pos = pos.clamp(s.start, s.end);
        }
        pos
    }

    fn pull_snapshot(&mut self) {
        let Ok(guard) = self.shared.lock() else { return };
        self.snap = guard.clone();
        drop(guard);
        // Once the player reports a fresh position, trust it over our guess.
        if let Some(o) = &self.seek_override {
            if self.snap.last_updated != o.last_updated_then {
                self.seek_override = None;
            }
        }
    }

    fn set_visible(&mut self, show: bool) {
        if show == self.visible {
            return;
        }
        self.visible = show;
        unsafe {
            if show {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                self.hover = Hit::None;
                self.pressed = Hit::None;
                if self.dragging {
                    self.dragging = false;
                    let _ = ReleaseCapture();
                }
            }
        }
    }

    fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % 10 == 1 {
            self.light = read_dword(THEME_KEY, w!("SystemUsesLightTheme")) == Some(1);
        }
        if self.seek_override.as_ref().is_some_and(|o| o.at.elapsed() > Duration::from_secs(3)) {
            self.seek_override = None;
        }

        let Some(tb) = locate_taskbar() else {
            self.set_visible(false);
            return;
        };

        // Make the taskbar our owner: an owned window always stays directly
        // above its owner in Z order, so the widget never slips behind the
        // taskbar and follows it behind full-screen apps.
        if tb.hwnd != self.owner {
            unsafe {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, tb.hwnd.0 as isize);
            }
            self.owner = tb.hwnd;
        }

        let tb_w = tb.rect.right - tb.rect.left;
        let tb_h = tb.rect.bottom - tb.rect.top;
        let horizontal = tb_w > tb_h;
        if !self.snap.active || !horizontal || tb_h < 16 || fullscreen_app_running(tb.hwnd) {
            self.set_visible(false);
            return;
        }

        let scale = tb.dpi as f32 / 96.0;
        let total = format_time(self.snap.end - self.snap.start);
        let layout = Layout::new(tb_h, scale, total.len());
        let w = layout.width.ceil() as i32;
        let x = (tb.tray_left - w - (2.0 * scale) as i32 + self.offset).max(tb.rect.left);
        let rect = (x, tb.rect.top, w, tb_h);
        if rect != self.rect {
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    rect.0,
                    rect.1,
                    rect.2,
                    rect.3,
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
                );
            }
            self.rect = rect;
            self.last_frame = None;
        }
        self.layout = layout;

        self.repaint();
        self.set_visible(true);
    }

    /// Redraw if anything visible changed since the last frame.
    fn repaint(&mut self) {
        let (_, _, w, h) = self.rect;
        if w <= 0 || h <= 0 || !self.snap.active {
            return;
        }
        let has_timeline = self.has_timeline();
        let duration = (self.snap.end - self.snap.start).max(1);
        let fraction = if self.dragging {
            self.drag_fraction
        } else {
            ((self.current_position() - self.snap.start) as f64 / duration as f64) as f32
        }
        .clamp(0.0, 1.0);
        let shown_pos = (fraction as f64 * duration as f64) as i64;

        let frame = Frame {
            w,
            h,
            light: self.light,
            title: self.snap.title.clone(),
            artist: self.snap.artist.clone(),
            art_gen: self.snap.art_gen,
            has_art: self.snap.art.is_some(),
            playing: self.snap.playing,
            can_prev: self.snap.can_prev,
            can_next: self.snap.can_next,
            can_toggle: self.snap.can_toggle,
            can_seek: self.snap.can_seek,
            has_timeline,
            elapsed: if has_timeline { format_time(shown_pos) } else { String::new() },
            total: if has_timeline { format_time(duration) } else { String::new() },
            fill_px: (fraction * self.layout.track.w).round() as i32,
            hover: self.hover,
            pressed: self.pressed,
            dragging: self.dragging,
        };
        if self.last_frame.as_ref() != Some(&frame) {
            let art = self.snap.art.clone();
            let spotify = self.snap.app_id.to_lowercase().contains("spotify");
            self.renderer.paint(
                self.hwnd,
                &frame,
                &self.layout,
                art.as_deref().map(|v| v.as_slice()),
                spotify,
            );
            self.last_frame = Some(frame);
        }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        self.layout.hit(x as f32, y as f32, self.has_timeline())
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        if !self.tracking_leave {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                self.tracking_leave = TrackMouseEvent(&mut tme).is_ok();
            }
        }
        if self.dragging {
            self.drag_fraction = self.layout.seek_fraction(x as f32);
        } else {
            self.hover = self.hit(x, y);
        }
        self.repaint();
    }

    fn mouse_leave(&mut self) {
        self.tracking_leave = false;
        if !self.dragging {
            self.hover = Hit::None;
            self.pressed = Hit::None;
        }
        self.repaint();
    }

    fn mouse_down(&mut self, x: i32, y: i32) {
        let hit = self.hit(x, y);
        self.hover = hit;
        self.pressed = hit;
        if hit == Hit::Seek && self.snap.can_seek {
            self.dragging = true;
            self.drag_fraction = self.layout.seek_fraction(x as f32);
            unsafe {
                SetCapture(self.hwnd);
            }
        }
        self.repaint();
    }

    /// Returns the app to open, if the click landed on the album art.
    fn mouse_up(&mut self, x: i32, y: i32) -> Option<String> {
        let mut open = None;
        if self.dragging {
            self.dragging = false;
            unsafe {
                let _ = ReleaseCapture();
            }
            self.drag_fraction = self.layout.seek_fraction(x as f32);
            let duration = self.snap.end - self.snap.start;
            let target = self.snap.start + (self.drag_fraction as f64 * duration as f64) as i64;
            let _ = self.tx.send(Cmd::Seek(target));
            self.seek_override = Some(SeekOverride {
                position: target,
                at: Instant::now(),
                last_updated_then: self.snap.last_updated,
            });
            self.hover = self.hit(x, y);
        } else {
            let hit = self.hit(x, y);
            if hit == self.pressed {
                match hit {
                    Hit::Art => open = Some(self.snap.app_id.clone()),
                    Hit::Prev if self.snap.can_prev => {
                        let _ = self.tx.send(Cmd::Prev);
                    }
                    Hit::Next if self.snap.can_next => {
                        let _ = self.tx.send(Cmd::Next);
                    }
                    Hit::Play if self.snap.can_toggle => {
                        let _ = self.tx.send(Cmd::Toggle);
                    }
                    _ => {}
                }
            }
            self.hover = hit;
        }
        self.pressed = Hit::None;
        self.repaint();
        open
    }

    fn cancel_drag(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.pressed = Hit::None;
            self.repaint();
        }
    }

    fn nudge(&mut self, delta: i32) {
        self.offset = (self.offset + delta).clamp(-4000, 4000);
        write_dword(SETTINGS_KEY, w!("Offset"), self.offset as u32);
        self.tick();
    }
}

fn show_menu(hwnd: HWND) {
    const ID_OPEN: usize = 1;
    const ID_AUTOSTART: usize = 2;
    const ID_RESET: usize = 3;
    const ID_EXIT: usize = 4;

    let Some((app_id, offset)) = with_app(|a| (a.snap.app_id.clone(), a.offset)) else { return };
    let autostart = autostart_enabled();

    // The menu runs its own message loop, so the app state must not be
    // borrowed while it is open.
    let choice = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN, w!("Open player"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let check = if autostart { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | check, ID_AUTOSTART, w!("Start with Windows"));
        let grey = if offset == 0 { MF_GRAYED } else { MF_ENABLED };
        let _ = AppendMenuW(menu, MF_STRING | grey, ID_RESET, w!("Reset position"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        choice.0 as usize
    };

    match choice {
        ID_OPEN => launch::open_app(&app_id),
        ID_AUTOSTART => set_autostart(!autostart),
        ID_RESET => {
            with_app(|a| a.nudge(-a.offset));
        }
        ID_EXIT => {
            QUITTING.store(true, Ordering::Relaxed);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        _ => {}
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_app(|a| a.tick());
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            with_app(|a| {
                a.pull_snapshot();
                a.tick();
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            with_app(|a| a.mouse_move(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(|a| a.mouse_leave());
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            with_app(|a| a.mouse_down(x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = point(lparam);
            // Launching can pump messages, so do it after the state is released.
            if let Some(Some(app_id)) = with_app(|a| a.mouse_up(x, y)) {
                launch::open_app(&app_id);
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_app(|a| a.cancel_drag());
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Shift + wheel nudges the widget sideways, in case it overlaps
            // something on a particular taskbar setup.
            if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                with_app(|a| a.nudge(if delta > 0 { 4 } else { -4 }));
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            WIDGET_HWND.store(0, Ordering::Relaxed);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    unsafe {
        // One instance only.
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarMediaWidget"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let Ok(module) = GetModuleHandleW(None) else { return };
        let class_name = w!("TaskbarMediaWidget");
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return;
        }

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<Cmd>();
        {
            let shared = shared.clone();
            std::thread::spawn(move || media::run(shared, rx, &WIDGET_HWND, WM_APP_UPDATE));
        }

        // If Explorer restarts, the taskbar (our owner window) is destroyed
        // and takes the widget with it, so recreate it until the user exits.
        loop {
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class_name,
                &HSTRING::from("Taskbar Media"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            ) else {
                return;
            };

            let offset = read_dword(SETTINGS_KEY, w!("Offset")).unwrap_or(0) as i32;
            APP.with(|cell| {
                let mut slot = cell.borrow_mut();
                let snap = shared.lock().map(|g| g.clone()).unwrap_or_default();
                *slot = Some(App {
                    hwnd,
                    owner: HWND::default(),
                    shared: shared.clone(),
                    tx: tx.clone(),
                    snap,
                    renderer: slot.take().map(|old| old.renderer).unwrap_or_else(Renderer::new),
                    layout: Layout::default(),
                    last_frame: None,
                    rect: (0, 0, 0, 0),
                    visible: false,
                    light: false,
                    ticks: 0,
                    offset: offset.clamp(-4000, 4000),
                    hover: Hit::None,
                    pressed: Hit::None,
                    dragging: false,
                    drag_fraction: 0.0,
                    tracking_leave: false,
                    seek_override: None,
                });
            });
            WIDGET_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
            SetTimer(Some(hwnd), TIMER_ID, TICK_MS, None);
            with_app(|a| a.tick());

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            if QUITTING.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    }
}
