//! Taskbar Media: a now-playing widget that sits on the Windows taskbar, just
//! left of the notification area / clock.
//!
//! Settings gear, album art (click to open the player), previous / play-pause
//! / next, then title and artist over a seek bar. Works with anything that
//! reports to Windows' media controls: Spotify, browsers, Media Player...
//!
//! One widget window is created per taskbar it should appear on. A hidden
//! controller window owns the timer and receives updates from the media
//! worker thread.

#![windows_subsystem = "windows"]

mod config;
mod launch;
mod media;
mod render;
mod settings;

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::CreateMutexW;
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

use config::{Config, DISPLAY_ALL, DISPLAY_ONE};
use media::{Cmd, Snapshot};
use render::{ArtCache, Frame, Hit, Layout, LayoutParams};

const WM_APP_UPDATE: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const TICK_MS: u32 = 200;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WIDGET_CLASS: PCWSTR = w!("TaskbarMediaWidget");

/// The controller window, which the media worker posts updates to.
static CONTROLLER_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` against the app state. Returns None if the state is already in use
/// further up the stack (a nested message), in which case the message is
/// simply skipped; the next timer tick catches up.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(f)))
}

/// Used by the settings window.
pub fn current_config() -> Config {
    with_app(|a| a.cfg.clone()).unwrap_or_else(Config::load)
}

/// Change settings, save them, and apply them straight away.
pub fn update_config(f: impl FnOnce(&mut Config)) {
    with_app(|a| {
        f(&mut a.cfg);
        a.cfg.save();
        a.apply_config();
    });
}

struct SeekOverride {
    position: i64,
    at: Instant,
    last_updated_then: i64,
}

/// One taskbar that exists right now.
#[derive(Clone, Copy)]
struct Taskbar {
    hwnd: HWND,
    rect: RECT,
    /// Left edge of the tray / clock area; the widget ends just before it.
    anchor: i32,
    dpi: u32,
    primary: bool,
}

/// One widget window, attached to one taskbar.
struct Widget {
    hwnd: HWND,
    taskbar: HWND,
    primary: bool,
    params: LayoutParams,
    layout: Layout,
    last_frame: Option<Frame>,
    art: Option<ArtCache>,
    rect: (i32, i32, i32, i32),
    visible: bool,
    hover: Hit,
    pressed: Hit,
    dragging: bool,
    drag_fraction: f32,
    tracking_leave: bool,
}

struct App {
    module: HINSTANCE,
    shared: Arc<Mutex<Snapshot>>,
    tx: Sender<Cmd>,
    snap: Snapshot,
    cfg: Config,
    widgets: Vec<Widget>,
    system_light: bool,
    ticks: u32,
    seek_override: Option<SeekOverride>,
}

/// What a click asks for, carried out once the app state is released.
enum Action {
    None,
    Open(String),
    Settings,
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

fn child_rect(parent: HWND, class: PCWSTR) -> Option<RECT> {
    let child = unsafe { FindWindowExW(Some(parent), None, class, None).ok()? };
    window_rect(child)
}

fn describe_taskbar(hwnd: HWND, primary: bool) -> Option<Taskbar> {
    let rect = window_rect(hwnd)?;
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        d => d,
    };
    let scale = dpi as f32 / 96.0;
    // The main taskbar has a real notification-area window. Windows 10's
    // extra taskbars have a clock window. Windows 11's extra taskbars draw
    // their clock without a window of its own, so its width is estimated
    // (Shift + mouse wheel nudges the widget if the estimate is off).
    let anchor = child_rect(hwnd, w!("TrayNotifyWnd"))
        .or_else(|| child_rect(hwnd, w!("ClockButton")))
        .map(|r| r.left)
        .unwrap_or(rect.right - ((if primary { 240.0 } else { 130.0 }) * scale) as i32);
    Some(Taskbar { hwnd, rect, anchor, dpi, primary })
}

fn monitor_device(hwnd: HWND) -> String {
    unsafe {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if !GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return String::new();
        }
        let n = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        String::from_utf16_lossy(&info.szDevice[..n])
    }
}

/// The taskbars the widget should currently appear on, per the settings.
fn wanted_taskbars(cfg: &Config) -> Vec<Taskbar> {
    let mut all = Vec::new();
    unsafe {
        if let Ok(main) = FindWindowW(w!("Shell_TrayWnd"), None) {
            all.extend(describe_taskbar(main, true));
        }
        let mut prev: Option<HWND> = None;
        for _ in 0..16 {
            match FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), None) {
                Ok(h) if !h.is_invalid() => {
                    all.extend(describe_taskbar(h, false));
                    prev = Some(h);
                }
                _ => break,
            }
        }
    }
    match cfg.display_mode {
        DISPLAY_ALL => all,
        DISPLAY_ONE => {
            let chosen = all.iter().find(|t| monitor_device(t.hwnd) == cfg.display_name).copied();
            // That display may be unplugged, or have no taskbar: use the main one.
            chosen.or_else(|| all.iter().find(|t| t.primary).copied()).into_iter().collect()
        }
        _ => all.into_iter().filter(|t| t.primary).collect(),
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
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "TaskbarMediaWidget"
        ) {
            return false;
        }
        let Some(wr) = window_rect(fg) else { return false };
        let monitor = MonitorFromWindow(taskbar, MONITOR_DEFAULTTONEAREST);
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

impl Widget {
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
}

impl App {
    fn has_timeline(&self) -> bool {
        self.snap.end > self.snap.start
    }

    fn light_text_on_dark(&self) -> bool {
        match self.cfg.text_color {
            1 => true,
            2 => false,
            _ => !self.system_light,
        }
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

    fn apply_config(&mut self) {
        media::SPOTIFY_ONLY.store(self.cfg.spotify_only, Ordering::Relaxed);
        for w in &mut self.widgets {
            w.last_frame = None;
        }
        self.tick();
    }

    fn index_of(&self, hwnd: HWND) -> Option<usize> {
        self.widgets.iter().position(|w| w.hwnd == hwnd)
    }

    fn create_widget(&mut self, tb: &Taskbar) {
        let created = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                WIDGET_CLASS,
                w!("Taskbar Media"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(self.module),
                None,
            )
        };
        // Make the taskbar the widget's owner: an owned window always stays
        // directly above its owner in Z order, so the widget never slips
        // behind the taskbar and follows it behind full-screen apps. Set
        // after creation (rather than passed to CreateWindowEx) so that our
        // input queue is not tied to Explorer's.
        if let Ok(hwnd) = created {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, tb.hwnd.0 as isize);
            }
        }
        if let Ok(hwnd) = created {
            self.widgets.push(Widget {
                hwnd,
                taskbar: tb.hwnd,
                primary: tb.primary,
                params: LayoutParams::default(),
                layout: Layout::default(),
                last_frame: None,
                art: None,
                rect: (0, 0, 0, 0),
                visible: false,
                hover: Hit::None,
                pressed: Hit::None,
                dragging: false,
                drag_fraction: 0.0,
                tracking_leave: false,
            });
        }
    }

    /// Keep one widget per wanted taskbar, positioned and painted.
    fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % 10 == 1 {
            self.system_light = config::system_light_theme();
        }
        if self.seek_override.as_ref().is_some_and(|o| o.at.elapsed() > Duration::from_secs(3)) {
            self.seek_override = None;
        }

        let bars = wanted_taskbars(&self.cfg);

        // Drop widgets whose taskbar is gone or no longer wanted. (When
        // Explorer restarts, Windows destroys the widgets along with their
        // owner taskbars; they are recreated below for the new ones.)
        self.widgets.retain(|w| unsafe {
            let alive = IsWindow(Some(w.hwnd)).as_bool();
            let keep = alive && bars.iter().any(|b| b.hwnd == w.taskbar);
            if alive && !keep {
                let _ = DestroyWindow(w.hwnd);
            }
            keep
        });
        for b in &bars {
            if !self.widgets.iter().any(|w| w.taskbar == b.hwnd) {
                self.create_widget(b);
            }
        }

        let showable = self.snap.active && !(self.cfg.hide_paused && !self.snap.playing);
        let time_chars = format_time(self.snap.end - self.snap.start).len();
        for i in 0..self.widgets.len() {
            let Some(tb) = bars.iter().find(|b| b.hwnd == self.widgets[i].taskbar).copied() else {
                continue;
            };
            let tb_w = tb.rect.right - tb.rect.left;
            let tb_h = tb.rect.bottom - tb.rect.top;
            // Only horizontal taskbars (bottom or top of the screen).
            if !showable || tb_w <= tb_h || tb_h < 16 || fullscreen_app_running(tb.hwnd) {
                self.widgets[i].set_visible(false);
                continue;
            }

            let scale = tb.dpi as f32 / 96.0;
            let params = LayoutParams {
                height: tb_h,
                scale,
                time_chars,
                text_size: self.cfg.text_size,
                width: self.cfg.width,
                show_times: self.cfg.show_times,
            };
            let offset = if tb.primary { self.cfg.offset } else { self.cfg.offset2 };
            let widget = &mut self.widgets[i];
            if widget.params != params {
                widget.params = params;
                widget.layout = Layout::new(&params);
            }
            let w = widget.layout.width.ceil() as i32;
            let x = (tb.anchor - w - (2.0 * scale) as i32 + offset)
                .min(tb.rect.right - w)
                .max(tb.rect.left);
            let rect = (x, tb.rect.top, w, tb_h);
            if rect != widget.rect {
                unsafe {
                    let _ = SetWindowPos(
                        widget.hwnd,
                        None,
                        rect.0,
                        rect.1,
                        rect.2,
                        rect.3,
                        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
                    );
                }
                widget.rect = rect;
                widget.last_frame = None;
            }
            self.repaint(i);
            self.widgets[i].set_visible(true);
        }
    }

    /// Redraw widget `i` if anything visible changed since its last frame.
    fn repaint(&mut self, i: usize) {
        let (_, _, w, h) = self.widgets[i].rect;
        if w <= 0 || h <= 0 || !self.snap.active {
            return;
        }
        let has_timeline = self.has_timeline();
        let duration = (self.snap.end - self.snap.start).max(1);
        let widget = &self.widgets[i];
        let fraction = if widget.dragging {
            widget.drag_fraction
        } else {
            ((self.current_position() - self.snap.start) as f64 / duration as f64) as f32
        }
        .clamp(0.0, 1.0);
        let shown_pos = (fraction as f64 * duration as f64) as i64;
        // Whole pixels, so the bar only repaints when it visibly moves.
        let track_w = widget.layout.track.w.max(1.0);
        let fill = (fraction * track_w).round() / track_w;

        let frame = Frame {
            w,
            h,
            params: widget.params,
            smooth: self.cfg.smooth,
            light: !self.light_text_on_dark(),
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
            fill,
            hover: widget.hover,
            pressed: widget.pressed,
            dragging: widget.dragging,
        };
        if widget.last_frame.as_ref() != Some(&frame) {
            let art = self.snap.art.clone();
            let spotify = self.snap.app_id.to_lowercase().contains("spotify");
            let widget = &mut self.widgets[i];
            render::paint(
                widget.hwnd,
                &frame,
                &mut widget.art,
                art.as_deref().map(|v| v.as_slice()),
                spotify,
            );
            widget.last_frame = Some(frame);
        }
    }

    fn hit(&self, i: usize, x: i32, y: i32) -> Hit {
        self.widgets[i].layout.hit(x as f32, y as f32, self.has_timeline())
    }

    fn mouse_move(&mut self, i: usize, x: i32, y: i32) {
        let hit = self.hit(i, x, y);
        let widget = &mut self.widgets[i];
        if !widget.tracking_leave {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: widget.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                widget.tracking_leave = TrackMouseEvent(&mut tme).is_ok();
            }
        }
        if widget.dragging {
            widget.drag_fraction = widget.layout.seek_fraction(x as f32);
        } else {
            widget.hover = hit;
        }
        self.repaint(i);
    }

    fn mouse_leave(&mut self, i: usize) {
        let widget = &mut self.widgets[i];
        widget.tracking_leave = false;
        if !widget.dragging {
            widget.hover = Hit::None;
            widget.pressed = Hit::None;
        }
        self.repaint(i);
    }

    fn mouse_down(&mut self, i: usize, x: i32, y: i32) {
        let hit = self.hit(i, x, y);
        let can_seek = self.snap.can_seek;
        let widget = &mut self.widgets[i];
        widget.hover = hit;
        widget.pressed = hit;
        if hit == Hit::Seek && can_seek {
            widget.dragging = true;
            widget.drag_fraction = widget.layout.seek_fraction(x as f32);
            unsafe {
                SetCapture(widget.hwnd);
            }
        }
        self.repaint(i);
    }

    fn mouse_up(&mut self, i: usize, x: i32, y: i32) -> Action {
        let mut action = Action::None;
        let hit = self.hit(i, x, y);
        if self.widgets[i].dragging {
            let widget = &mut self.widgets[i];
            widget.dragging = false;
            unsafe {
                let _ = ReleaseCapture();
            }
            widget.drag_fraction = widget.layout.seek_fraction(x as f32);
            widget.hover = hit;
            let fraction = widget.drag_fraction as f64;
            let duration = self.snap.end - self.snap.start;
            let target = self.snap.start + (fraction * duration as f64) as i64;
            let _ = self.tx.send(Cmd::Seek(target));
            self.seek_override = Some(SeekOverride {
                position: target,
                at: Instant::now(),
                last_updated_then: self.snap.last_updated,
            });
        } else {
            if hit == self.widgets[i].pressed {
                match hit {
                    Hit::Gear => action = Action::Settings,
                    Hit::Art => action = Action::Open(self.snap.app_id.clone()),
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
            self.widgets[i].hover = hit;
        }
        self.widgets[i].pressed = Hit::None;
        // A seek moves the bar on every widget, not just this one.
        for j in 0..self.widgets.len() {
            if self.widgets[j].visible {
                self.repaint(j);
            }
        }
        action
    }

    fn cancel_drag(&mut self, i: usize) {
        if self.widgets[i].dragging {
            self.widgets[i].dragging = false;
            self.widgets[i].pressed = Hit::None;
            self.repaint(i);
        }
    }

    /// Shift + wheel: slide this taskbar's widget sideways.
    fn nudge(&mut self, i: usize, delta: i32) {
        let slot = if self.widgets[i].primary { &mut self.cfg.offset } else { &mut self.cfg.offset2 };
        *slot = (*slot + delta).clamp(-4000, 4000);
        self.cfg.save();
        self.tick();
    }
}

fn show_menu(hwnd: HWND) {
    const ID_OPEN: usize = 1;
    const ID_SETTINGS: usize = 2;
    const ID_EXIT: usize = 3;

    let Some((app_id, module)) = with_app(|a| (a.snap.app_id.clone(), a.module)) else { return };

    // The menu runs its own message loop, so the app state must not be
    // borrowed while it is open.
    let choice = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN, w!("Open player"));
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Settings..."));
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
        ID_SETTINGS => settings::open(module),
        ID_EXIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// Run `f` for the widget that owns `hwnd`.
fn with_widget<R>(hwnd: HWND, f: impl FnOnce(&mut App, usize) -> R) -> Option<R> {
    with_app(|a| a.index_of(hwnd).map(|i| f(a, i))).flatten()
}

unsafe extern "system" fn widget_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            with_widget(hwnd, |a, i| a.mouse_move(i, x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_widget(hwnd, |a, i| a.mouse_leave(i));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            with_widget(hwnd, |a, i| a.mouse_down(i, x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = point(lparam);
            // Launching or opening a window pumps messages, so do it after
            // the app state has been released.
            let action = with_widget(hwnd, |a, i| (a.mouse_up(i, x, y), a.module));
            match action {
                Some((Action::Open(app_id), _)) => launch::open_app(&app_id),
                Some((Action::Settings, module)) => settings::open(module),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_widget(hwnd, |a, i| a.cancel_drag(i));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                with_widget(hwnd, |a, i| a.nudge(i, if delta > 0 { 4 } else { -4 }));
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn controller_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
        settings::enable_visual_styles();
        render::init();

        let Ok(module) = GetModuleHandleW(None) else { return };
        let module: HINSTANCE = module.into();

        let widget_class = WNDCLASSW {
            lpfnWndProc: Some(widget_proc),
            hInstance: module,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: WIDGET_CLASS,
            ..Default::default()
        };
        let controller_class = WNDCLASSW {
            lpfnWndProc: Some(controller_proc),
            hInstance: module,
            lpszClassName: w!("TaskbarMediaController"),
            ..Default::default()
        };
        if RegisterClassW(&widget_class) == 0 || RegisterClassW(&controller_class) == 0 {
            return;
        }
        settings::register_class(module);

        // Hidden message-only window: timer + updates from the worker.
        let Ok(controller) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("TaskbarMediaController"),
            w!("Taskbar Media"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module),
            None,
        ) else {
            return;
        };

        let cfg = Config::load();
        media::SPOTIFY_ONLY.store(cfg.spotify_only, Ordering::Relaxed);

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<Cmd>();
        CONTROLLER_HWND.store(controller.0 as isize, Ordering::Relaxed);
        {
            let shared = shared.clone();
            std::thread::spawn(move || media::run(shared, rx, &CONTROLLER_HWND, WM_APP_UPDATE));
        }

        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                module,
                shared,
                tx,
                snap: Snapshot::default(),
                cfg,
                widgets: Vec::new(),
                system_light: false,
                ticks: 0,
                seek_override: None,
            });
        });
        SetTimer(Some(controller), TIMER_ID, TICK_MS, None);
        with_app(|a| a.tick());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Tab / Enter / Esc handling for the settings window.
            let dialog = settings::hwnd();
            if !dialog.is_invalid() && IsDialogMessageW(dialog, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
