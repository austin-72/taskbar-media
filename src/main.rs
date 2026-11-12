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
mod crash;
mod devices;
mod launch;
mod media;
mod probe;
mod render;
mod reserve;
mod settings;
mod tray;
mod update;
mod volume;

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, GlobalFree, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    POINT, RECT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, ScreenToClient, MonitorFromWindow, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SendInput, SetCapture, TrackMouseEvent, INPUT, INPUT_0,
    INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, TME_LEAVE, TRACKMOUSEEVENT,
    VK_ESCAPE, VK_SHIFT, VK_VOLUME_DOWN, VK_VOLUME_UP,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, ShellExecuteW, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use config::{Config, DISPLAY_ALL, DISPLAY_ONE};
use media::{Cmd, Snapshot};
use render::{ArtCache, Card, Frame, Hit, Layout, LayoutParams};

const WM_APP_UPDATE: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const TICK_MS: u32 = 200;
/// Fast timer that only runs while a title is scrolling.
const ANIM_TIMER_ID: usize = 2;
const ANIM_MS: u32 = 33;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WIDGET_CLASS: PCWSTR = w!("TaskbarMediaWidget");

/// The controller window, which the media worker posts updates to.
static CONTROLLER_HWND: AtomicIsize = AtomicIsize::new(0);
/// The "TaskbarCreated" message Explorer broadcasts when it (re)starts.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

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

/// Save pictures of the widget and its popups as PNG files in a "pictures"
/// folder beside the exe. Returns the folder and how many were saved.
pub fn save_pictures() -> Option<(std::path::PathBuf, usize)> {
    let dir = std::env::current_exe().ok()?.parent()?.join("pictures");
    let saved = with_app(|a| a.save_pictures(&dir))?;
    Some((dir, saved))
}

/// A picture of the widget for the settings window (see `App::preview`).
pub fn preview(max_w: i32, other_taskbars: bool) -> Option<(Vec<u32>, i32, i32, u32)> {
    with_app(|a| a.preview(max_w, other_taskbars)).flatten()
}

/// Re-read every setting from the registry and apply it (after an import or
/// a reset).
pub fn reload_config() {
    with_app(|a| {
        a.cfg = Config::load();
        a.apply_config();
    });
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
    /// False when `anchor` is only an estimate.
    anchor_exact: bool,
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
    /// How much wider the title line is than its space (0 or less: it fits).
    overflow: f32,
    /// The text `scroll_start` belongs to.
    scroll_key: String,
    scroll_start: Instant,
}

struct App {
    module: HINSTANCE,
    controller: HWND,
    tray_shown: bool,
    tray_tip: String,
    anim_on: bool,
    /// The hover card's window, what it last showed, and whether it is up.
    card: HWND,
    card_frame: Option<Card>,
    card_visible: bool,
    /// The widget the mouse is resting on, and since when.
    hover_since: Option<(usize, Instant)>,
    /// A click put the card away; it stays away until the mouse leaves.
    card_dismissed: bool,
    /// Windows 10: space can be reserved on the main taskbar.
    can_reserve: bool,
    shared: Arc<Mutex<Snapshot>>,
    tx: Sender<Cmd>,
    snap: Snapshot,
    cfg: Config,
    widgets: Vec<Widget>,
    system_light: bool,
    /// The Windows accent color, 0xAARRGGBB.
    system_accent: u32,
    ticks: u32,
    seek_override: Option<SeekOverride>,
    /// Volume level to show for a moment after the wheel changed it.
    volume_flash: Option<(u8, Instant)>,
    /// (level 0..=1, muted) for the volume button, while it is on a widget.
    volume: Option<(f32, bool)>,
    /// The volume mixer popup: its window and, while open, its contents.
    /// Row 0 is the system volume (key ""), the rest are apps.
    mixer: HWND,
    mixer_open: bool,
    /// The same window doubles as the output device list.
    popup: Popup,
    outputs_rows: Vec<devices::Device>,
    outputs_current: String,
    outputs_hover: Option<usize>,
    outputs_frame: Option<render::Picker>,
    menu_rows: Vec<MenuRow>,
    menu_hover: Option<usize>,
    menu_frame: Option<render::MenuPanel>,
    mixer_rows: Vec<volume::MixerEntry>,
    mixer_hover: Option<(usize, MixerPart)>,
    mixer_drag: Option<usize>,
    mixer_frame: Option<render::Mixer>,
    /// DPI scale, and the spot it hangs from: the button's centre x, and the
    /// widget's top and bottom.
    mixer_scale: f32,
    mixer_anchor: (i32, i32, i32),
    /// It has had keyboard focus (so losing focus should close it).
    mixer_was_active: bool,
    /// Since when the mouse has been away from it and the widget.
    mixer_away: Option<Instant>,
    /// Message to show for a moment in place of the title.
    notice: Option<(String, Instant)>,
    /// The current audio output, while the output button is on the widget.
    output: Option<devices::Device>,
}

/// What the popup above the widget is showing.
#[derive(Clone, Copy, PartialEq)]
enum Popup {
    /// The volume mixer.
    Mixer,
    /// The list of output devices.
    Outputs,
    /// The right-click menu.
    Menu,
}

/// What a row of the right-click menu does.
#[derive(Clone, PartialEq)]
enum MenuCmd {
    None,
    Update,
    Open,
    Copy,
    /// Let the widget choose which player to follow.
    Auto,
    /// Follow the player with this app id.
    Player(String),
    Mixer,
    Outputs,
    Settings,
    Exit,
}

#[derive(Clone)]
struct MenuRow {
    text: String,
    /// 0 a command, 1 a section heading, 2 a dividing line.
    kind: u8,
    tick: bool,
    strong: bool,
    cmd: MenuCmd,
}

/// A part of a row in the volume mixer popup.
#[derive(Clone, Copy, PartialEq)]
enum MixerPart {
    /// The speaker icon: click to mute.
    Icon,
    Slider,
}

/// What a click asks for, carried out once the app state is released.
enum Action {
    None,
    Open(String),
    Settings,
    /// Switch between the two chosen output devices.
    ToggleOutput,
    /// Open the releases page in the browser.
    Releases,
    Exit,
}

/// Carry out what a click asked for. Call with the app state released.
fn perform(action: Action, module: HINSTANCE) {
    match action {
        Action::None => {}
        Action::Open(app_id) => launch::open_app(&app_id),
        Action::Settings => settings::open(module),
        Action::ToggleOutput => toggle_output(),
        Action::Releases => open_releases_page(),
        Action::Exit => unsafe { PostQuitMessage(0) },
    }
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
    // extra taskbars have a clock window. Windows 11's extra taskbars have
    // neither; there the position comes from `probe`, or failing that from
    // this estimate (Shift + mouse wheel nudges the widget if it is off).
    let exact = child_rect(hwnd, w!("TrayNotifyWnd"))
        .or_else(|| child_rect(hwnd, w!("ClockButton")))
        .map(|r| r.left);
    let estimate = rect.right - ((if primary { 240.0 } else { 130.0 }) * scale) as i32;
    Some(Taskbar {
        hwnd,
        rect,
        anchor: exact.unwrap_or(estimate),
        anchor_exact: exact.is_some(),
        dpi,
        primary,
    })
}

/// True when the taskbar has slid (mostly) off its screen, as an auto-hiding
/// taskbar does.
fn taskbar_hidden(tb: &Taskbar) -> bool {
    unsafe {
        let monitor = MonitorFromWindow(tb.hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
            return false;
        }
        let visible = tb.rect.bottom.min(mi.rcMonitor.bottom) - tb.rect.top.max(mi.rcMonitor.top);
        visible < (tb.rect.bottom - tb.rect.top) / 2
    }
}

/// Press and release a volume key, as a keyboard's volume buttons would.
fn volume_step(up: bool) {
    let vk = if up { VK_VOLUME_UP } else { VK_VOLUME_DOWN };
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    };
    unsafe {
        SendInput(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
    }
}

/// Where a too-long title is in its scroll cycle: wait, scroll to the end,
/// wait, scroll back.
fn marquee_offset(overflow: f32, elapsed: f32, speed: f32) -> i32 {
    const HOLD_START: f32 = 2.5;
    const HOLD_END: f32 = 1.8;
    let travel = overflow / speed;
    let t = elapsed % (HOLD_START + travel + HOLD_END + travel);
    let fraction = if t < HOLD_START {
        0.0
    } else if t < HOLD_START + travel {
        (t - HOLD_START) / travel
    } else if t < HOLD_START + travel + HOLD_END {
        1.0
    } else {
        1.0 - (t - HOLD_START - travel - HOLD_END) / travel
    };
    (fraction * overflow).round() as i32
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

fn is_shell_or_own_window(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    let class = String::from_utf16_lossy(&class[..n]);
    matches!(
        class.as_str(),
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "TaskbarMediaWidget"
            | "TaskbarMediaCard"
            | "TaskbarMediaMixer"
    )
}

fn covers_monitor(window: HWND, monitor_of: HWND) -> bool {
    unsafe {
        let Some(wr) = window_rect(window) else { return false };
        let monitor = MonitorFromWindow(monitor_of, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        wr.left <= m.left && wr.top <= m.top && wr.right >= m.right && wr.bottom >= m.bottom
    }
}

/// True while something full-screen (a game, a video, a slideshow) is
/// covering this taskbar, which is when the widget must get out of the way
/// too.
fn fullscreen_over(tb: &Taskbar) -> bool {
    unsafe {
        // The direct test: look at a spot in the taskbar's clock area. If
        // the window showing there is not the taskbar but one that fills the
        // whole monitor, the taskbar is covered. This catches browser video,
        // which the shell does not report as full-screen.
        let scale = tb.dpi as f32 / 96.0;
        let probe = POINT {
            x: (tb.anchor + (12.0 * scale) as i32).min(tb.rect.right - 3).max(tb.rect.left),
            y: (tb.rect.top + tb.rect.bottom) / 2,
        };
        let at_probe = GetAncestor(WindowFromPoint(probe), GA_ROOT);
        if !at_probe.is_invalid()
            && at_probe != tb.hwnd
            && !is_shell_or_own_window(at_probe)
            && covers_monitor(at_probe, tb.hwnd)
        {
            return true;
        }

        // And what the shell itself reports (exclusive full-screen games).
        // A maximised window also fills the monitor when the taskbar
        // auto-hides, hence asking the shell rather than trusting the size.
        let fg = GetForegroundWindow();
        !fg.is_invalid()
            && fg != tb.hwnd
            && !is_shell_or_own_window(fg)
            && covers_monitor(fg, tb.hwnd)
            && SHQueryUserNotificationState()
                .is_ok_and(|s| s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN)
    }
}

/// True if `taskbar` has ended up in front of `widget`. The widget is meant
/// to sit directly above the taskbar, but the shell re-raises the taskbar at
/// times (leaving full-screen is one) and can leave the widget underneath.
fn taskbar_in_front(widget: HWND, taskbar: HWND) -> bool {
    unsafe {
        let mut above = GetWindow(widget, GW_HWNDPREV);
        for _ in 0..500 {
            match above {
                Ok(h) if h == taskbar => return true,
                Ok(h) if !h.is_invalid() => above = GetWindow(h, GW_HWNDPREV),
                _ => return false,
            }
        }
        false
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
    /// The volume button's icon shows the system volume rather than the player's.
    fn volume_is_system(&self) -> bool {
        self.cfg.volume_target == 1
    }

    fn refresh_volume(&mut self) {
        self.volume = if self.cfg.shows(config::ITEM_VOLUME) && self.snap.active {
            volume::read(&self.snap.app_id, self.volume_is_system())
        } else {
            None
        };
    }

    /// Whether the seek bar is on screen: the player reports a timeline and
    /// the chosen text layout has a seek bar row.
    fn seek_shown(&self) -> bool {
        self.has_timeline() && self.cfg.text_layout < 3
    }

    fn has_timeline(&self) -> bool {
        self.snap.end > self.snap.start
    }

    /// The text color (0xFFRRGGBB), and whether it is a dark color meant
    /// for a light taskbar.
    fn text_color(&self) -> (u32, bool) {
        match self.cfg.text_color {
            1 => (0xFFFF_FFFF, false),
            2 => (0xFF00_0000, true),
            3 => {
                let c = self.cfg.text_rgb;
                let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
                (c | 0xFF00_0000, (r * 299 + g * 587 + b * 114) / 1000 < 128)
            }
            _ if self.system_light => (0xFF00_0000, true),
            _ => (0xFFFF_FFFF, false),
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

    fn tray_tooltip(&self) -> String {
        if !self.snap.active || self.snap.title.is_empty() {
            "Taskbar Media".to_string()
        } else if self.snap.artist.is_empty() {
            self.snap.title.clone()
        } else {
            format!("{} - {}", self.snap.title, self.snap.artist)
        }
    }

    /// Add, remove or relabel the tray icon to match the settings and track.
    /// `force` re-adds it, for after Explorer restarts and forgets it.
    fn sync_tray(&mut self, force: bool) {
        let tip = self.tray_tooltip();
        if !self.cfg.tray_icon {
            if self.tray_shown {
                tray::remove(self.controller);
                self.tray_shown = false;
            }
        } else if !self.tray_shown || force {
            tray::add(self.controller, self.module, &tip);
            self.tray_shown = true;
            self.tray_tip = tip;
        } else if tip != self.tray_tip {
            tray::update_tip(self.controller, &tip);
            self.tray_tip = tip;
        }
    }

    fn apply_config(&mut self) {
        media::SPOTIFY_ONLY.store(self.cfg.spotify_only, Ordering::Relaxed);
        update::ENABLED.store(self.cfg.check_updates, Ordering::Relaxed);
        self.sync_tray(false);
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
                overflow: 0.0,
                scroll_key: String::new(),
                scroll_start: Instant::now(),
            });
        }
    }

    /// Keep one widget per wanted taskbar, positioned and painted.
    fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % 10 == 1 {
            self.system_light = config::system_light_theme();
            // The output can change behind our back (a headset is plugged
            // in, or the user switches in Windows), so look every 2 seconds.
            self.output = if self.cfg.shows(config::ITEM_OUTPUT) && self.snap.active {
                devices::current()
            } else {
                None
            };
            self.system_accent = config::system_accent().unwrap_or(0xFF4C_C2FF);
        }
        if self.seek_override.as_ref().is_some_and(|o| o.at.elapsed() > Duration::from_secs(3)) {
            self.seek_override = None;
        }
        // The volume can be changed elsewhere (the player, the Windows
        // mixer, the keyboard), so re-read it about once a second.
        if self.ticks % 5 == 1 {
            self.refresh_volume();
        }
        self.tick_mixer();
        if self.volume_flash.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(1500)) {
            self.volume_flash = None;
        }
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(1800)) {
            self.notice = None;
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

        let mut reserved = false;
        let showable = self.snap.active && !(self.cfg.hide_paused && !self.snap.playing);
        for i in 0..self.widgets.len() {
            let Some(tb) = bars.iter().find(|b| b.hwnd == self.widgets[i].taskbar).copied() else {
                continue;
            };
            let tb_w = tb.rect.right - tb.rect.left;
            let tb_h = tb.rect.bottom - tb.rect.top;
            // Only horizontal taskbars (bottom or top of the screen).
            if !showable
                || tb_w <= tb_h
                || tb_h < 16
                || taskbar_hidden(&tb)
                || fullscreen_over(&tb)
            {
                self.widgets[i].set_visible(false);
                continue;
            }

            let scale = tb.dpi as f32 / 96.0;
            let measured = probe::get(tb.hwnd);
            let anchor = if tb.anchor_exact {
                tb.anchor
            } else {
                // Believe the measurement only if it is in the right-hand half.
                measured
                    .tray_left
                    .filter(|x| *x > tb.rect.left + tb_w / 2 && *x < tb.rect.right)
                    .unwrap_or(tb.anchor)
            };
            let offset = if tb.primary { self.cfg.offset } else { self.cfg.offset2 };
            let right = anchor - (2.0 * scale) as i32 + offset;

            // The space between the last app button and the tray, if known
            // (Windows 11; on Windows 10 space is reserved further down).
            let room = measured
                .apps_right
                .filter(|x| self.cfg.avoid_apps && *x > tb.rect.left && *x < right)
                .map(|x| (right - x) as f32 - 8.0 * scale);

            // Use the chosen width, or the widest smaller one that fits.
            let base = self.layout_params(tb_h, scale, tb.primary);
            let mut params = base;
            let mut layout = Layout::new(&params);
            if let Some(room) = room {
                let mut width = self.cfg.width;
                while layout.width > room {
                    if width > 0 {
                        width -= 1;
                        params = LayoutParams { width, ..base };
                    } else if !params.mini {
                        params = LayoutParams { width: 0, mini: true, ..base };
                    } else {
                        break;
                    }
                    layout = Layout::new(&params);
                }
            }

            let widget = &mut self.widgets[i];
            if widget.params != params {
                widget.params = params;
                widget.layout = layout;
            }
            let w = widget.layout.width.ceil() as i32;
            let x = (right - w).min(tb.rect.right - w).max(tb.rect.left);
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
            // Windows 10: have Explorer keep its app buttons out of the way.
            if self.can_reserve && self.cfg.avoid_apps && tb.primary {
                reserved |= reserve::apply(tb.hwnd, x - (4.0 * scale) as i32);
            }
            self.repaint(i);
            self.widgets[i].set_visible(true);
            // Put the widget back on top if the taskbar got in front of it.
            if taskbar_in_front(self.widgets[i].hwnd, tb.hwnd) {
                unsafe {
                    let _ = SetWindowPos(
                        self.widgets[i].hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
        if self.can_reserve && !reserved {
            if let Some(main) = bars.iter().find(|b| b.primary) {
                reserve::restore(main.hwnd);
            } else if let Ok(main) = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) } {
                reserve::restore(main);
            }
        }

        self.update_card();

        // Run the fast timer only while some visible title needs to scroll.
        let scrolling = self.cfg.scroll_titles
            && self.widgets.iter().any(|w| w.visible && !w.params.mini && w.overflow > 0.0);
        if scrolling != self.anim_on {
            self.anim_on = scrolling;
            unsafe {
                if scrolling {
                    SetTimer(Some(self.controller), ANIM_TIMER_ID, ANIM_MS, None);
                } else {
                    let _ = KillTimer(Some(self.controller), ANIM_TIMER_ID);
                }
            }
        }
    }

    // ---- the volume mixer popup --------------------------------------------

    /// The system volume followed by every app in the Windows mixer.
    fn load_mixer_rows() -> Vec<volume::MixerEntry> {
        let (level, muted) = volume::read("", true).unwrap_or((1.0, false));
        let mut rows = vec![volume::MixerEntry { key: String::new(), name: "System".to_string(), level, muted }];
        rows.extend(volume::mixer());
        rows
    }

    /// Open the mixer above widget `i`'s volume button, or close it.
    fn toggle_mixer(&mut self, i: usize) {
        self.toggle_popup(i, Popup::Mixer);
    }

    /// Open the output device list above widget `i`'s output button, or
    /// close it.
    fn toggle_outputs(&mut self, i: usize) {
        self.toggle_popup(i, Popup::Outputs);
    }

    fn load_outputs(&mut self) {
        self.outputs_rows = devices::list();
        self.outputs_current = devices::current().map(|d| d.id).unwrap_or_default();
    }

    /// Open the popup as `kind` above the matching button of widget `i`. If
    /// it is already open as that kind, close it instead.
    fn toggle_popup(&mut self, i: usize, kind: Popup) {
        if self.mixer_open {
            let same = self.popup == kind;
            self.close_mixer();
            if same {
                return;
            }
        }
        if i >= self.widgets.len() {
            return;
        }
        let widget = &self.widgets[i];
        let (wx, wy, _, wh) = widget.rect;
        let button = if kind == Popup::Mixer { widget.layout.vol_icon } else { widget.layout.output };
        let anchor = (wx + button.cx() as i32, wy, wy + wh);
        let scale = widget.params.scale.max(0.5);
        self.open_popup(kind, anchor, scale);
    }

    /// Open the right-click menu at the mouse: above widget `i`, or, from
    /// the tray icon, above the mouse itself.
    fn open_menu(&mut self, i: Option<usize>) {
        if self.mixer_open {
            self.close_mixer();
        }
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let widget = i.and_then(|i| self.widgets.get(i));
        let anchor = match widget {
            Some(w) => (pt.x, w.rect.1, w.rect.1 + w.rect.3),
            None => (pt.x, pt.y - 10, pt.y + 10),
        };
        let scale = widget
            .or(self.widgets.first())
            .map(|w| w.params.scale)
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() }.max(96) as f32 / 96.0);
        self.open_popup(Popup::Menu, anchor, scale.max(0.5));
    }

    /// The rows of the right-click menu for the current state.
    fn build_menu(&self) -> Vec<MenuRow> {
        let item = |text: &str, cmd: MenuCmd| MenuRow { text: text.to_string(), kind: 0, tick: false, strong: false, cmd };
        let divider = || MenuRow { text: String::new(), kind: 2, tick: false, strong: false, cmd: MenuCmd::None };
        let mut rows = Vec::new();
        if let Some(version) = update::available() {
            rows.push(MenuRow { strong: true, ..item(&format!("Get version {version}"), MenuCmd::Update) });
            rows.push(divider());
        }
        if self.snap.active {
            rows.push(item(&format!("Open {}", launch::display_name(&self.snap.app_id)), MenuCmd::Open));
            if !self.snap.title.is_empty() {
                rows.push(item("Copy track name", MenuCmd::Copy));
            }
            rows.push(divider());
        }
        // With more than one player around, let the user pick.
        if self.snap.sessions.len() > 1 {
            let preferred = media::preferred();
            rows.push(MenuRow { kind: 1, ..item("PLAYER", MenuCmd::None) });
            rows.push(MenuRow { tick: preferred.is_none(), ..item("Automatic", MenuCmd::Auto) });
            for id in self.snap.sessions.iter().take(12) {
                rows.push(MenuRow {
                    tick: preferred.as_ref() == Some(id),
                    ..item(&launch::display_name(id), MenuCmd::Player(id.clone()))
                });
            }
            rows.push(divider());
        }
        rows.push(item("Volume mixer", MenuCmd::Mixer));
        rows.push(item("Output device", MenuCmd::Outputs));
        rows.push(divider());
        rows.push(item("Settings", MenuCmd::Settings));
        rows.push(item("Exit", MenuCmd::Exit));
        rows
    }

    /// Show the popup as `kind`, hanging from `anchor` (centre x, and the
    /// top and bottom of whatever it belongs to).
    fn open_popup(&mut self, kind: Popup, anchor: (i32, i32, i32), scale: f32) {
        self.popup = kind;
        self.mixer_scale = scale;
        self.mixer_anchor = anchor;
        match kind {
            Popup::Mixer => self.mixer_rows = Self::load_mixer_rows(),
            Popup::Outputs => {
                self.load_outputs();
                if self.outputs_rows.is_empty() {
                    return;
                }
            }
            Popup::Menu => self.menu_rows = self.build_menu(),
        }
        self.outputs_hover = None;
        self.outputs_frame = None;
        self.menu_hover = None;
        self.menu_frame = None;
        self.mixer_hover = None;
        self.mixer_drag = None;
        self.mixer_frame = None;
        self.mixer_was_active = false;
        self.mixer_away = None;
        self.mixer_open = true;
        self.hide_card();
        self.show_mixer();
        // Take focus, so that Esc and clicking elsewhere close it.
        unsafe {
            let _ = SetForegroundWindow(self.mixer);
            self.mixer_was_active = GetForegroundWindow() == self.mixer;
        }
    }

    fn close_mixer(&mut self) {
        if !self.mixer_open {
            return;
        }
        self.mixer_open = false;
        self.mixer_drag = None;
        self.mixer_frame = None;
        self.outputs_frame = None;
        self.menu_frame = None;
        unsafe {
            let _ = ShowWindow(self.mixer, SW_HIDE);
        }
        self.refresh_volume();
        for j in 0..self.widgets.len() {
            if self.widgets[j].visible {
                self.repaint(j);
            }
        }
    }

    /// Draw the mixer if anything changed, and keep it placed above (or,
    /// for a taskbar at the top, below) the volume button.
    /// The highlight color the popups use (0 means the text color).
    fn popup_accent(&self) -> u32 {
        match self.cfg.accent_mode {
            2 => 0,
            3 => self.cfg.accent_rgb,
            _ => self.system_accent,
        }
    }

    /// The mixer as it should be drawn now.
    fn mixer_picture(&self) -> render::Mixer {
        render::Mixer {
            scale: self.mixer_scale,
            smooth: self.cfg.smooth,
            light: self.system_light,
            font: self.cfg.font,
            accent: self.popup_accent(),
            rows: self
                .mixer_rows
                .iter()
                .map(|r| render::MixerRow {
                    name: r.name.clone(),
                    level: (r.level * 100.0).round() as u8,
                    muted: r.muted,
                })
                .collect(),
            hover_row: self.mixer_hover.map_or(-1, |(row, _)| row as i32),
            hover_icon: self.mixer_hover.is_some_and(|(_, part)| part == MixerPart::Icon),
            drag_row: self.mixer_drag.map_or(-1, |row| row as i32),
        }
    }

    /// The output device list as it should be drawn now.
    fn outputs_picture(&self) -> render::Picker {
        render::Picker {
            scale: self.mixer_scale,
            smooth: self.cfg.smooth,
            light: self.system_light,
            font: self.cfg.font,
            accent: self.popup_accent(),
            rows: self
                .outputs_rows
                .iter()
                .map(|d| render::PickerRow {
                    name: d.name.clone(),
                    headphones: d.headphones,
                    current: d.id == self.outputs_current,
                })
                .collect(),
            hover_row: self.outputs_hover.map_or(-1, |row| row as i32),
        }
    }

    /// The right-click menu as it should be drawn now.
    fn menu_picture(&self) -> render::MenuPanel {
        render::MenuPanel {
            scale: self.mixer_scale,
            smooth: self.cfg.smooth,
            light: self.system_light,
            font: self.cfg.font,
            accent: self.popup_accent(),
            rows: self
                .menu_rows
                .iter()
                .map(|r| render::MenuItem { text: r.text.clone(), kind: r.kind, tick: r.tick, strong: r.strong })
                .collect(),
            hover_row: self.menu_hover.map_or(-1, |row| row as i32),
        }
    }

    /// The hover card for the current track.
    fn card_picture(&self, scale: f32) -> Card {
        let (w, h) = render::card_size(scale);
        let sample = !self.snap.active;
        let status = {
            let app = if sample { "Player".to_string() } else { launch::display_name(&self.snap.app_id) };
            let state = if sample || self.snap.playing { "Playing" } else { "Paused" };
            if self.has_timeline() {
                format!("{app}  \u{00B7}  {state}  \u{00B7}  {}", format_time(self.snap.end - self.snap.start))
            } else {
                format!("{app}  \u{00B7}  {state}")
            }
        };
        let or_sample = |text: &str, stand_in: &str| if sample { stand_in.to_string() } else { text.to_string() };
        Card {
            w,
            h,
            scale,
            smooth: self.cfg.smooth,
            light: self.system_light,
            font: self.cfg.font,
            title: or_sample(&self.snap.title, "Song title"),
            artist: or_sample(&self.snap.artist, "Artist"),
            album: or_sample(&self.snap.album, "Album"),
            status,
            art_gen: self.snap.art_gen,
            has_art: self.snap.art.is_some(),
        }
    }

    /// Save pictures of the widget and each of its popups, drawn by the
    /// same code that draws them on screen, as PNG files in `dir`. They are
    /// drawn at twice the size of the main display's scale so that they stay
    /// sharp on a web page. Returns how many files were written.
    fn save_pictures(&mut self, dir: &std::path::Path) -> usize {
        if std::fs::create_dir_all(dir).is_err() {
            return 0;
        }
        let base = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() }.max(96) as f32 / 96.0;
        let scale = base * 2.0;
        let pad = (20.0 * scale) as i32;
        let backdrop = if self.system_light { 0x00C9_D8F0 } else { 0x0023_2A44 };
        let art = self.snap.art.clone();
        let art = art.as_deref().map(|v| v.as_slice());
        let spotify = self.snap.app_id.to_lowercase().contains("spotify");
        let mut saved = 0;

        // The widget, on a strip the color of the taskbar.
        let mut params = self.layout_params((40.0 * scale) as i32, scale, true);
        if !self.snap.active {
            params.extra = true;
            params.players = true;
            params.time_chars = 4;
        }
        let layout = Layout::new(&params);
        let w = layout.width.ceil() as i32;
        let frame = self.make_frame(params, &layout, w, params.height, None);
        let taskbar = if frame.light { 0x00E9_EDF3 } else { 0x0018_191D };
        let pixels = render::render_preview(&frame, art, spotify, taskbar);
        saved += usize::from(render::save_png(&dir.join("widget.png"), &pixels, w, params.height, true, taskbar, 0));

        // The popups, each on a plain backdrop. Their contents are loaded
        // fresh and the popup's own state is put back afterwards.
        let keep = (self.mixer_scale, self.mixer_hover, self.outputs_hover, self.menu_hover);
        let rows = (
            std::mem::take(&mut self.mixer_rows),
            std::mem::take(&mut self.outputs_rows),
            std::mem::take(&mut self.menu_rows),
            std::mem::take(&mut self.outputs_current),
        );
        self.mixer_scale = scale;
        (self.mixer_hover, self.outputs_hover, self.menu_hover) = (None, None, None);
        self.mixer_rows = Self::load_mixer_rows();
        self.load_outputs();
        self.menu_rows = self.build_menu();

        let card = self.card_picture(scale);
        let (mixer, outputs, menu) = (self.mixer_picture(), self.outputs_picture(), self.menu_picture());
        let shots = [
            ("card.png", render::Shot::Card(&card, art, spotify)),
            ("mixer.png", render::Shot::Mixer(&mixer)),
            ("outputs.png", render::Shot::Picker(&outputs)),
            ("menu.png", render::Shot::Menu(&menu)),
        ];
        for (name, shot) in shots {
            if let Some((pixels, w, h)) = render::snapshot(shot) {
                saved += usize::from(render::save_png(&dir.join(name), &pixels, w, h, false, backdrop, pad));
            }
        }

        (self.mixer_scale, self.mixer_hover, self.outputs_hover, self.menu_hover) = keep;
        (self.mixer_rows, self.outputs_rows, self.menu_rows, self.outputs_current) = rows;
        saved
    }

    fn show_mixer(&mut self) {
        let frame = self.mixer_picture();
        let layout = render::mixer_layout(self.mixer_scale, self.mixer_rows.len());
        let picker = self.outputs_picture();
        let menu = self.menu_picture();
        let (w, h) = match self.popup {
            Popup::Mixer => (layout.w, layout.h),
            Popup::Outputs => {
                let (w, h, _) = render::picker_layout(self.mixer_scale, self.outputs_rows.len());
                (w, h)
            }
            Popup::Menu => {
                let kinds: Vec<u8> = self.menu_rows.iter().map(|r| r.kind).collect();
                let (w, h, _) = render::menu_layout(self.mixer_scale, &kinds);
                (w, h)
            }
        };
        let (cx, top, bottom) = self.mixer_anchor;
        let gap = (8.0 * self.mixer_scale) as i32;
        let (mut x, mut y) = (cx - w / 2, top - h - gap);
        unsafe {
            let monitor = MonitorFromPoint(POINT { x: cx, y: top }, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                let m = mi.rcMonitor;
                if y < m.top {
                    y = bottom + gap;
                }
                x = x.min(m.right - w - gap).max(m.left + gap);
            }
            match self.popup {
                Popup::Mixer if self.mixer_frame.as_ref() != Some(&frame) => {
                    render::paint_mixer(self.mixer, &frame);
                    self.mixer_frame = Some(frame);
                }
                Popup::Outputs if self.outputs_frame.as_ref() != Some(&picker) => {
                    render::paint_picker(self.mixer, &picker);
                    self.outputs_frame = Some(picker);
                }
                Popup::Menu if self.menu_frame.as_ref() != Some(&menu) => {
                    render::paint_menu(self.mixer, &menu);
                    self.menu_frame = Some(menu);
                }
                _ => {}
            }
            let _ = SetWindowPos(
                self.mixer,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    /// Called on every tick: keep the open mixer current, and close it once
    /// it has lost focus.
    fn tick_mixer(&mut self) {
        if !self.mixer_open {
            return;
        }
        unsafe {
            let focused = GetForegroundWindow() == self.mixer;
            if focused {
                self.mixer_was_active = true;
            } else if self.mixer_was_active && self.mixer_drag.is_none() {
                self.close_mixer();
                return;
            } else {
                // It never got focus (Windows can refuse), so it cannot tell
                // when the user clicks elsewhere. Close it once the mouse has
                // been away from it and the widgets for a couple of seconds.
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let inside = |r: (i32, i32, i32, i32)| {
                    pt.x >= r.0 && pt.x < r.0 + r.2 && pt.y >= r.1 && pt.y < r.1 + r.3
                };
                let near = window_rect(self.mixer)
                    .is_some_and(|r| inside((r.left, r.top, r.right - r.left, r.bottom - r.top)))
                    || self.widgets.iter().any(|w| w.visible && inside(w.rect));
                if near || self.mixer_drag.is_some() {
                    self.mixer_away = None;
                } else if self.mixer_away.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) {
                    self.close_mixer();
                    return;
                }
            }
        }
        match self.popup {
            // Apps start and stop, and volumes change elsewhere.
            Popup::Mixer if self.mixer_drag.is_none() && self.ticks % 2 == 0 => {
                self.mixer_rows = Self::load_mixer_rows();
            }
            // Devices get plugged in and unplugged.
            Popup::Outputs if self.ticks % 5 == 0 => self.load_outputs(),
            _ => {}
        }
        self.show_mixer();
    }

    fn mixer_hit(&self, x: i32, y: i32) -> Option<(usize, MixerPart)> {
        let layout = render::mixer_layout(self.mixer_scale, self.mixer_rows.len());
        let (x, y) = (x as f32, y as f32);
        let reach = 8.0 * self.mixer_scale;
        layout.rows.iter().enumerate().find_map(|(i, r)| {
            if !r.row.contains(x, y) {
                None
            } else if r.icon.contains(x, y) {
                Some((i, MixerPart::Icon))
            } else if x >= r.track.x - reach && x < r.track.x + r.track.w + reach {
                Some((i, MixerPart::Slider))
            } else {
                None
            }
        })
    }

    /// Apply a volume level (0..=1) to a mixer row.
    fn mixer_set_level(&mut self, row: usize, level: f32) {
        let level = level.clamp(0.0, 1.0);
        let Some(entry) = self.mixer_rows.get_mut(row) else { return };
        // Moving the slider of something muted un-mutes it.
        let unmute = entry.muted && level > 0.0;
        entry.level = level;
        if unmute {
            entry.muted = false;
        }
        let key = entry.key.clone();
        if key.is_empty() {
            if unmute {
                volume::set_mute("", true, false);
            }
            volume::set_level("", true, level);
        } else {
            if unmute {
                volume::mixer_set_mute(&key, false);
            }
            volume::mixer_set_level(&key, level);
        }
    }

    fn mixer_level_at(&self, row: usize, x: i32) -> f32 {
        let layout = render::mixer_layout(self.mixer_scale, self.mixer_rows.len());
        layout.rows.get(row).map_or(0.0, |r| ((x as f32 - r.track.x) / r.track.w.max(1.0)).clamp(0.0, 1.0))
    }

    /// The output list row at a point in the popup.
    fn outputs_hit(&self, x: i32, y: i32) -> Option<usize> {
        let (_, _, rows) = render::picker_layout(self.mixer_scale, self.outputs_rows.len());
        rows.iter().position(|r| r.contains(x as f32, y as f32))
    }

    /// Make `device` the default output and say so on the widget.
    fn apply_output(&mut self, device: &devices::Device) {
        let message = if devices::set_default(&device.id) {
            format!("Output: {}", device.name)
        } else {
            "Could not switch output".to_string()
        };
        if self.output.is_some() {
            self.output = devices::current();
        }
        self.notice = Some((message, Instant::now()));
    }

    /// The command row of the menu at a point in the popup.
    fn menu_hit(&self, x: i32, y: i32) -> Option<usize> {
        let kinds: Vec<u8> = self.menu_rows.iter().map(|r| r.kind).collect();
        let (_, _, rows) = render::menu_layout(self.mixer_scale, &kinds);
        rows.iter()
            .position(|r| r.contains(x as f32, y as f32))
            .filter(|row| self.menu_rows[*row].kind == 0)
    }

    /// Carry out a row of the right-click menu.
    fn run_menu(&mut self, row: usize) -> Action {
        let Some(cmd) = self.menu_rows.get(row).map(|r| r.cmd.clone()) else { return Action::None };
        let (anchor, scale) = (self.mixer_anchor, self.mixer_scale);
        self.close_mixer();
        match cmd {
            MenuCmd::None => Action::None,
            MenuCmd::Update => Action::Releases,
            MenuCmd::Open => Action::Open(self.snap.app_id.clone()),
            MenuCmd::Settings => Action::Settings,
            MenuCmd::Exit => Action::Exit,
            MenuCmd::Copy => {
                let text = if self.snap.artist.is_empty() {
                    self.snap.title.clone()
                } else {
                    format!("{} - {}", self.snap.artist, self.snap.title)
                };
                let done = copy_to_clipboard(self.controller, &text);
                let message = if done { "Copied to clipboard" } else { "Could not copy" };
                self.notice = Some((message.to_string(), Instant::now()));
                self.tick();
                Action::None
            }
            MenuCmd::Auto | MenuCmd::Player(_) => {
                media::set_preferred(match cmd {
                    MenuCmd::Player(id) => Some(id),
                    _ => None,
                });
                let _ = self.tx.send(Cmd::Refresh);
                Action::None
            }
            // These two take the menu's place.
            MenuCmd::Mixer => {
                self.open_popup(Popup::Mixer, anchor, scale);
                Action::None
            }
            MenuCmd::Outputs => {
                self.open_popup(Popup::Outputs, anchor, scale);
                Action::None
            }
        }
    }

    fn mixer_mouse_move(&mut self, x: i32, y: i32) {
        if self.popup == Popup::Menu {
            self.menu_hover = self.menu_hit(x, y);
            self.show_mixer();
            return;
        }
        if self.popup == Popup::Outputs {
            self.outputs_hover = self.outputs_hit(x, y);
            self.show_mixer();
            return;
        }
        match self.mixer_drag {
            Some(row) => {
                let level = self.mixer_level_at(row, x);
                self.mixer_set_level(row, level);
            }
            None => self.mixer_hover = self.mixer_hit(x, y),
        }
        self.show_mixer();
    }

    fn mixer_mouse_down(&mut self, x: i32, y: i32) -> Action {
        if self.popup == Popup::Menu {
            return match self.menu_hit(x, y) {
                Some(row) => self.run_menu(row),
                None => Action::None,
            };
        }
        if self.popup == Popup::Outputs {
            // Choosing a device switches to it and closes the list.
            if let Some(device) = self.outputs_hit(x, y).and_then(|row| self.outputs_rows.get(row).cloned()) {
                self.apply_output(&device);
                self.close_mixer();
                self.tick();
            }
            return Action::None;
        }
        match self.mixer_hit(x, y) {
            Some((row, MixerPart::Slider)) => {
                self.mixer_drag = Some(row);
                unsafe {
                    SetCapture(self.mixer);
                }
                let level = self.mixer_level_at(row, x);
                self.mixer_set_level(row, level);
            }
            Some((row, MixerPart::Icon)) => {
                if let Some(entry) = self.mixer_rows.get_mut(row) {
                    entry.muted = !entry.muted;
                    let (key, mute) = (entry.key.clone(), entry.muted);
                    if key.is_empty() {
                        volume::set_mute("", true, mute);
                    } else {
                        volume::mixer_set_mute(&key, mute);
                    }
                }
            }
            None => {}
        }
        self.show_mixer();
        Action::None
    }

    fn mixer_mouse_up(&mut self) {
        if self.mixer_drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            self.show_mixer();
        }
    }

    /// Wheel over a row: 4% per notch.
    fn mixer_wheel(&mut self, x: i32, y: i32, up: bool) {
        if self.popup != Popup::Mixer {
            return;
        }
        let layout = render::mixer_layout(self.mixer_scale, self.mixer_rows.len());
        let row = layout.rows.iter().position(|r| r.row.contains(x as f32, y as f32));
        if let Some(row) = row {
            let level = self.mixer_rows[row].level + if up { 0.04 } else { -0.04 };
            self.mixer_set_level(row, level);
            self.show_mixer();
        }
    }

    fn hide_card(&mut self) {
        if self.card_visible {
            self.card_visible = false;
            unsafe {
                let _ = ShowWindow(self.card, SW_HIDE);
            }
        }
    }

    /// Show the hover card once the mouse has rested on a widget for a
    /// moment; hide it when the mouse leaves.
    fn update_card(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let over = self.widgets.iter().position(|w| {
            let (x, y, width, height) = w.rect;
            w.visible && !w.dragging && pt.x >= x && pt.x < x + width && pt.y >= y && pt.y < y + height
        });
        let allowed = self.cfg.hover_card
            && self.snap.active
            && !self.mixer_open;
        if over.is_none() {
            self.card_dismissed = false;
        }
        let Some(i) = over.filter(|_| allowed && !self.card_dismissed) else {
            self.hover_since = None;
            self.hide_card();
            return;
        };
        match self.hover_since {
            Some((widget, _)) if widget == i => {}
            _ => self.hover_since = Some((i, Instant::now())),
        }
        let rested = self.hover_since.is_some_and(|(_, since)| since.elapsed() >= Duration::from_millis(650));
        if !rested || self.widgets[i].pressed != Hit::None {
            self.hide_card();
            return;
        }

        let scale = self.widgets[i].params.scale.max(0.5);
        let frame = self.card_picture(scale);
        let (w, h) = (frame.w, frame.h);

        // Centred above the widget (below it for a taskbar at the top),
        // kept on the widget's monitor.
        let (wx, wy, ww, wh) = self.widgets[i].rect;
        let gap = (8.0 * scale) as i32;
        let (mut x, mut y) = (wx + ww / 2 - w / 2, wy - h - gap);
        unsafe {
            let monitor = MonitorFromWindow(self.widgets[i].hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                let m = mi.rcMonitor;
                if y < m.top {
                    y = wy + wh + gap;
                }
                x = x.min(m.right - w - gap).max(m.left + gap);
            }
            if self.card_frame.as_ref() != Some(&frame) {
                let art = self.snap.art.clone();
                let spotify = self.snap.app_id.to_lowercase().contains("spotify");
                render::paint_card(self.card, &frame, art.as_deref().map(|v| v.as_slice()), spotify);
                self.card_frame = Some(frame);
            }
            let _ = SetWindowPos(
                self.card,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        self.card_visible = true;
    }

    /// Advance scrolling titles.
    fn animate(&mut self) {
        for i in 0..self.widgets.len() {
            if self.widgets[i].visible && self.widgets[i].overflow > 0.0 {
                self.repaint(i);
            }
        }
    }

    /// Everything a widget of this layout and size shows right now, with no
    /// mouse interaction. `position` overrides the played fraction (while
    /// the seek bar is being dragged). With nothing playing, a sample track
    /// stands in, so that the settings preview always has something to show.
    fn make_frame(&self, params: LayoutParams, layout: &Layout, w: i32, h: i32, position: Option<f32>) -> Frame {
        let sample = !self.snap.active;
        let (title, artist, album) = if sample {
            ("Song title".to_string(), "Artist".to_string(), "Album".to_string())
        } else {
            (self.snap.title.clone(), self.snap.artist.clone(), self.snap.album.clone())
        };
        let has_timeline = if sample { self.cfg.text_layout < 3 } else { self.seek_shown() };
        let duration = if sample { 222 * 10_000_000 } else { (self.snap.end - self.snap.start).max(1) };
        let fraction = position
            .unwrap_or_else(|| {
                if sample {
                    0.38
                } else {
                    ((self.current_position() - self.snap.start) as f64 / duration as f64) as f32
                }
            })
            .clamp(0.0, 1.0);
        let shown_pos = (fraction as f64 * duration as f64) as i64;
        // Whole pixels, so the bar only repaints when it visibly moves.
        let track_w = layout.track.w.max(1.0);
        let fill = (fraction * track_w).round() / track_w;

        // Which text goes where, per the chosen text layout.
        let (line1, inline, below) = match self.cfg.text_layout {
            1 => (title, String::new(), artist),
            2 if !artist.is_empty() => (artist, title, String::new()),
            2 => (title, String::new(), String::new()),
            3 => (title, String::new(), artist),
            4 if !album.is_empty() => (title, String::new(), album),
            4 => (title, String::new(), artist),
            // "Title · Artist" needs the seek bar row; without one the
            // artist drops to the second row.
            _ if has_timeline => (title, artist, String::new()),
            _ => (title, String::new(), artist),
        };
        let (fg, light) = self.text_color();

        Frame {
            w,
            h,
            params,
            smooth: self.cfg.smooth,
            light,
            fg,
            title: line1,
            artist: inline,
            below,
            art_gen: self.snap.art_gen,
            has_art: self.snap.art.is_some(),
            playing: sample || self.snap.playing,
            can_prev: sample || self.snap.can_prev,
            can_next: sample || self.snap.can_next,
            can_toggle: sample || self.snap.can_toggle,
            can_seek: sample || self.snap.can_seek,
            has_timeline,
            elapsed: if has_timeline { format_time(shown_pos) } else { String::new() },
            total: if has_timeline { format_time(duration) } else { String::new() },
            fill,
            hover: Hit::None,
            pressed: Hit::None,
            dragging: false,
            scroll: false,
            scroll_px: 0,
            can_shuffle: sample || self.snap.can_shuffle,
            shuffle: self.snap.shuffle,
            can_repeat: sample || self.snap.can_repeat,
            repeat: self.snap.repeat,
            // Windows' accent and a custom color are both a fixed color.
            accent: if self.cfg.accent_mode == 3 { 1 } else { self.cfg.accent_mode },
            accent_rgb: if self.cfg.accent_mode == 3 { self.cfg.accent_rgb } else { self.system_accent },
            art_shape: self.cfg.art_shape,
            background: self.cfg.background,
            font: self.cfg.font,
            volume: self.volume_flash.as_ref().map(|(level, _)| *level),
            notice: self.notice.as_ref().map(|(text, _)| text.clone()),
            update: update::available().is_some(),
            headphones: self.output.as_ref().is_some_and(|d| d.headphones),
            vol: self.volume.map_or(if sample { 70 } else { 0 }, |(level, _)| (level * 100.0).round() as u8),
            vol_muted: self.volume.is_some_and(|(_, muted)| muted),
            mixer_open: self.mixer_open && self.popup == Popup::Mixer,
            outputs_open: self.mixer_open && self.popup == Popup::Outputs,
            players_enabled: sample || self.snap.sessions.len() > 1,
        }
    }

    /// The layout settings as they apply to one taskbar.
    fn layout_params(&self, height: i32, scale: f32, main_taskbar: bool) -> LayoutParams {
        LayoutParams {
            height,
            scale,
            time_chars: format_time(self.snap.end - self.snap.start).len(),
            text_size: self.cfg.text_size,
            width: self.cfg.width,
            show_times: self.cfg.show_times,
            mini: false,
            // Buttons that cannot be used right now are shown greyed out,
            // unless the user prefers them left out.
            extra: !self.cfg.hide_unusable || self.snap.can_shuffle || self.snap.can_repeat,
            players: !self.cfg.hide_unusable || self.snap.sessions.len() > 1,
            gap: self.cfg.button_gap,
            stack: self.cfg.stack_extras,
            order: self.cfg.order(main_taskbar),
            button_size: self.cfg.button_size,
            seek_size: self.cfg.seek_size,
        }
    }

    /// A picture of the widget for the settings window: opaque pixels, its
    /// width and height, and the taskbar-like color behind it. Scaled down
    /// if it would be wider than `max_w`.
    fn preview(&self, max_w: i32, other_taskbars: bool) -> Option<(Vec<u32>, i32, i32, u32)> {
        // Size it like the real thing on the main taskbar, when there is one.
        let real = self.widgets.iter().find(|w| w.primary && w.params.height > 0);
        let (mut height, mut scale) = match real {
            Some(w) => (w.params.height, w.params.scale),
            None => {
                let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() }.max(96) as f32 / 96.0;
                ((40.0 * scale) as i32, scale)
            }
        };
        // With nothing playing every button is shown, as a sample.
        let build = |height: i32, scale: f32| {
            let mut params = self.layout_params(height, scale, !other_taskbars);
            if !self.snap.active {
                params.extra = true;
                params.players = true;
                params.time_chars = 4;
            }
            let layout = Layout::new(&params);
            (params, layout)
        };
        let (mut params, mut layout) = build(height, scale);
        if layout.width > max_w as f32 && layout.width > 1.0 {
            let shrink = max_w as f32 / layout.width;
            height = ((height as f32 * shrink) as i32).max(16);
            scale *= shrink;
            (params, layout) = build(height, scale);
        }
        let w = (layout.width.ceil() as i32).min(max_w).max(1);
        let frame = self.make_frame(params, &layout, w, height, None);
        let background = if frame.light { 0x00E9_EDF3 } else { 0x0018_191D };
        let art = self.snap.art.clone();
        let spotify = self.snap.app_id.to_lowercase().contains("spotify");
        let pixels = render::render_preview(&frame, art.as_deref().map(|v| v.as_slice()), spotify, background);
        (!pixels.is_empty()).then_some((pixels, w, height, background))
    }

    /// Redraw widget `i` if anything visible changed since its last frame.
    fn repaint(&mut self, i: usize) {
        let (_, _, w, h) = self.widgets[i].rect;
        if w <= 0 || h <= 0 || !self.snap.active {
            return;
        }
        // Title scrolling: restart the cycle whenever the text changes.
        let scroll = self.cfg.scroll_titles && !self.widgets[i].params.mini;
        let key = format!("{}\u{1}{}", self.snap.title, self.snap.artist);
        let widget = &mut self.widgets[i];
        if widget.scroll_key != key {
            widget.scroll_key = key;
            widget.scroll_start = Instant::now();
            widget.overflow = 0.0;
        }
        let scroll_px = if scroll && widget.overflow > 0.0 {
            let speed = 28.0 * widget.params.scale.max(0.5);
            marquee_offset(widget.overflow, widget.scroll_start.elapsed().as_secs_f32(), speed)
        } else {
            0
        };

        let widget = &self.widgets[i];
        let dragged = widget.dragging.then_some(widget.drag_fraction);
        let mut frame = self.make_frame(widget.params, &widget.layout, w, h, dragged);
        frame.hover = widget.hover;
        frame.pressed = widget.pressed;
        frame.dragging = widget.dragging;
        frame.scroll = scroll;
        frame.scroll_px = scroll_px;
        if widget.last_frame.as_ref() != Some(&frame) {
            let art = self.snap.art.clone();
            let spotify = self.snap.app_id.to_lowercase().contains("spotify");
            let widget = &mut self.widgets[i];
            let overflow = render::paint(
                widget.hwnd,
                &frame,
                &mut widget.art,
                art.as_deref().map(|v| v.as_slice()),
                spotify,
            );
            widget.overflow = if scroll { overflow } else { 0.0 };
            widget.last_frame = Some(frame);
        }
    }

    fn hit(&self, i: usize, x: i32, y: i32) -> Hit {
        self.widgets[i].layout.hit(x as f32, y as f32, self.seek_shown())
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
        self.hover_since = None;
        self.card_dismissed = false;
        self.hide_card();
    }

    fn mouse_down(&mut self, i: usize, x: i32, y: i32) {
        let hit = self.hit(i, x, y);
        let can_seek = self.snap.can_seek;
        self.card_dismissed = true;
        self.hide_card();
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
                    Hit::VolIcon => self.toggle_mixer(i),
                    Hit::PlayerUp | Hit::PlayerDown => {
                        // Step through the players, wrapping at the ends.
                        let ids = &self.snap.sessions;
                        if ids.len() > 1 {
                            let at = ids.iter().position(|id| *id == self.snap.app_id).unwrap_or(0);
                            let next = if hit == Hit::PlayerDown {
                                (at + 1) % ids.len()
                            } else {
                                (at + ids.len() - 1) % ids.len()
                            };
                            let name = launch::display_name(&ids[next]);
                            media::set_preferred(Some(ids[next].clone()));
                            let _ = self.tx.send(Cmd::Refresh);
                            self.notice = Some((name, Instant::now()));
                        }
                    }
                    Hit::Output => {
                        let both = !self.cfg.output_a.is_empty() && !self.cfg.output_b.is_empty();
                        if self.cfg.output_click == 1 && both {
                            action = Action::ToggleOutput;
                        } else {
                            self.toggle_outputs(i);
                        }
                    }
                    Hit::Art => match self.cfg.art_click {
                        0 => action = Action::Open(self.snap.app_id.clone()),
                        1 if self.snap.can_toggle => {
                            let _ = self.tx.send(Cmd::Toggle);
                        }
                        _ => {}
                    },
                    Hit::Title if self.cfg.title_click == 1 => {
                        action = Action::Open(self.snap.app_id.clone());
                    }
                    Hit::Prev if self.snap.can_prev => {
                        let _ = self.tx.send(Cmd::Prev);
                    }
                    Hit::Next if self.snap.can_next => {
                        let _ = self.tx.send(Cmd::Next);
                    }
                    Hit::Play if self.snap.can_toggle => {
                        let _ = self.tx.send(Cmd::Toggle);
                    }
                    Hit::Shuffle if self.snap.can_shuffle => {
                        let _ = self.tx.send(Cmd::Shuffle(!self.snap.shuffle));
                    }
                    Hit::Repeat if self.snap.can_repeat => {
                        // Off -> repeat all -> repeat one -> off.
                        let next = match self.snap.repeat {
                            0 => 2,
                            2 => 1,
                            _ => 0,
                        };
                        let _ = self.tx.send(Cmd::Repeat(next));
                    }
                    Hit::Title if self.cfg.title_click == 0 && !self.snap.title.is_empty() => {
                        let text = if self.snap.artist.is_empty() {
                            self.snap.title.clone()
                        } else {
                            format!("{} - {}", self.snap.artist, self.snap.title)
                        };
                        let done = copy_to_clipboard(self.widgets[i].hwnd, &text);
                        let message = if done { "Copied to clipboard" } else { "Could not copy" };
                        self.notice = Some((message.to_string(), Instant::now()));
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

/// Flip between the two output devices chosen in settings.
fn toggle_output() {
    let Some((a, b)) = with_app(|app| (app.cfg.output_a.clone(), app.cfg.output_b.clone())) else {
        return;
    };
    let current = devices::current().map(|d| d.id).unwrap_or_default();
    let target = if current == a { b } else { a };
    match devices::list().into_iter().find(|d| d.id == target) {
        Some(device) => switch_output(&device),
        None => {
            with_app(|app| {
                app.notice = Some(("That device is not connected".to_string(), Instant::now()));
                app.tick();
            });
        }
    }
}

/// Make `device` the default output and say so on the widget.
fn switch_output(device: &devices::Device) {
    with_app(|app| {
        app.apply_output(device);
        app.tick();
    });
}

/// Open the latest release on GitHub in the default browser.
pub fn open_releases_page() {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(update::RELEASES_URL), None, None, SW_SHOWNORMAL);
    }
}

/// Put text on the clipboard. Returns false if the clipboard was busy.
fn copy_to_clipboard(owner: HWND, text: &str) -> bool {
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let mut done = false;
        if EmptyClipboard().is_ok() {
            if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
                let target = GlobalLock(memory) as *mut u16;
                if !target.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                    let _ = GlobalUnlock(memory);
                    // On success the clipboard owns the memory.
                    done = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))).is_ok();
                }
                if !done {
                    let _ = GlobalFree(Some(memory));
                }
            }
        }
        let _ = CloseClipboard();
        done
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
            if let Some((action, module)) = with_widget(hwnd, |a, i| (a.mouse_up(i, x, y), a.module)) {
                perform(action, module);
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_widget(hwnd, |a, i| a.cancel_drag(i));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            // Right-clicking the output button always lists the devices.
            let (x, y) = point(lparam);
            if with_widget(hwnd, |a, i| a.hit(i, x, y)) == Some(Hit::Output) {
                with_widget(hwnd, |a, i| {
                    // Always the list, even when a click is set to switch.
                    if !(a.mixer_open && a.popup == Popup::Outputs) {
                        a.toggle_outputs(i);
                    }
                });
            } else {
                with_widget(hwnd, |a, i| a.open_menu(Some(i)));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Shift + wheel nudges the widget; a plain wheel changes volume.
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                with_widget(hwnd, |a, i| a.nudge(i, if delta > 0 { 4 } else { -4 }));
            } else if delta != 0 {
                // 1: system volume. 2: the player's own volume, falling back
                // to system volume when the player has no mixer entry.
                let (mode, app_id) = with_app(|a| (a.cfg.wheel_mode, a.snap.app_id.clone()))
                    .unwrap_or((0, String::new()));
                let level = if mode == 2 { volume::adjust(&app_id, delta > 0) } else { None };
                match level {
                    Some(level) => {
                        with_app(|a| {
                            a.volume_flash = Some((level, Instant::now()));
                            a.refresh_volume();
                            a.tick();
                        });
                    }
                    None if mode != 0 => volume_step(delta > 0),
                    None => {}
                }
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn mixer_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            // Ask to be told when the mouse leaves, to clear the highlight.
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
            with_app(|a| a.mixer_mouse_move(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                if a.mixer_open && a.mixer_drag.is_none() {
                    a.mixer_hover = None;
                    a.outputs_hover = None;
                    a.menu_hover = None;
                    a.show_mixer();
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            // Opening settings and the like pumps messages, so do it after
            // the app state has been released.
            if let Some((action, module)) = with_app(|a| (a.mixer_mouse_down(x, y), a.module)) {
                perform(action, module);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_app(|a| a.mixer_mouse_up());
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_app(|a| a.mixer_drag = None);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Wheel messages carry screen coordinates.
            let (sx, sy) = point(lparam);
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            if delta != 0 {
                with_app(|a| a.mixer_wheel(pt.x, pt.y, delta > 0));
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            with_app(|a| a.close_mixer());
            LRESULT(0)
        }
        // Lost focus: the user clicked somewhere else.
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            with_app(|a| {
                if a.mixer_was_active {
                    a.close_mixer();
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn card_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn controller_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            if wparam.0 == ANIM_TIMER_ID {
                with_app(|a| a.animate());
            } else {
                with_app(|a| a.tick());
            }
            LRESULT(0)
        }
        // Windows is shutting down or the user is signing out: leave
        // cleanly, so that it is not mistaken for a crash.
        WM_ENDSESSION if wparam.0 != 0 => {
            with_app(|a| {
                if a.tray_shown {
                    tray::remove(a.controller);
                }
            });
            std::process::exit(0);
        }
        WM_APP_UPDATE => {
            with_app(|a| {
                a.pull_snapshot();
                a.sync_tray(false);
                a.tick();
            });
            LRESULT(0)
        }
        tray::WM_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP => {
                    if let Some(module) = with_app(|a| a.module) {
                        settings::open(module);
                    }
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    with_app(|a| a.open_menu(None));
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted: the tray icon has to be added again.
            with_app(|a| a.sync_tray(true));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The process the user starts. It shows nothing itself: it runs the widget
/// as a second copy of this exe and starts it again if it crashes.
fn supervise() {
    unsafe {
        // One instance only.
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarMediaWidget"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut crashes: Vec<Instant> = Vec::new();
        loop {
            match std::process::Command::new(&exe).arg(WORKER_FLAG).status() {
                // Exit code 0: the user chose Exit.
                Ok(status) if status.success() => return,
                // Ended from Task Manager (1), or by Windows shutting down:
                // deliberate, so not something to undo.
                Ok(status) if matches!(status.code().map(|c| c as u32), Some(1 | 0x4001_0004 | 0xC000_013A)) => {
                    return;
                }
                Ok(status) => {
                    let code = status.code().map_or("unknown".to_string(), |c| format!("{:#010x}", c as u32));
                    crash::write(&format!("the widget stopped unexpectedly (exit code {code})"));
                }
                Err(error) => {
                    crash::write(&format!("could not start the widget: {error}"));
                    return;
                }
            }
            // Something that crashes over and over is better left stopped.
            crashes.retain(|at| at.elapsed() < Duration::from_secs(300));
            crashes.push(Instant::now());
            if crashes.len() >= 5 {
                crash::write("crashed 5 times in 5 minutes; not restarting again");
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
            crash::write("restarting the widget");
        }
    }
}

const WORKER_FLAG: &str = "--widget";

fn main() {
    crash::install();
    if std::env::args().any(|arg| arg == WORKER_FLAG) {
        run_widget();
    } else {
        supervise();
    }
}

/// The widget itself.
fn run_widget() {
    unsafe {
        // One widget process only.
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarMediaWidgetProcess"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        settings::init_controls();
        render::init();

        let Ok(module) = GetModuleHandleW(None) else { return };
        let module: HINSTANCE = module.into();

        let app_icon = LoadIconW(Some(module), PCWSTR(1 as *const u16)).unwrap_or_default();
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
            hIcon: app_icon,
            lpszClassName: w!("TaskbarMediaController"),
            ..Default::default()
        };
        let card_class = WNDCLASSW {
            lpfnWndProc: Some(card_proc),
            hInstance: module,
            lpszClassName: w!("TaskbarMediaCard"),
            ..Default::default()
        };
        let mixer_class = WNDCLASSW {
            lpfnWndProc: Some(mixer_proc),
            hInstance: module,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("TaskbarMediaMixer"),
            ..Default::default()
        };
        if RegisterClassW(&widget_class) == 0
            || RegisterClassW(&controller_class) == 0
            || RegisterClassW(&card_class) == 0
            || RegisterClassW(&mixer_class) == 0
        {
            return;
        }
        settings::register_class(module);

        // Hidden window that owns the timers and the tray icon and receives
        // updates from the worker. (Not a message-only window: those do not
        // get the "TaskbarCreated" broadcast.)
        let Ok(controller) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("TaskbarMediaController"),
            w!("Taskbar Media"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module),
            None,
        ) else {
            return;
        };
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        probe::spawn();

        // The hover card: never takes focus, and clicks pass through it.
        let card = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TRANSPARENT,
            w!("TaskbarMediaCard"),
            w!("Taskbar Media"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module),
            None,
        )
        .unwrap_or_default();

        // The volume mixer popup: unlike the card it takes clicks and focus.
        let mixer = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("TaskbarMediaMixer"),
            w!("Taskbar Media volume"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module),
            None,
        )
        .unwrap_or_default();

        let cfg = Config::load();
        media::SPOTIFY_ONLY.store(cfg.spotify_only, Ordering::Relaxed);
        update::ENABLED.store(cfg.check_updates, Ordering::Relaxed);

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<Cmd>();
        CONTROLLER_HWND.store(controller.0 as isize, Ordering::Relaxed);
        {
            let shared = shared.clone();
            std::thread::spawn(move || media::run(shared, rx, &CONTROLLER_HWND, WM_APP_UPDATE));
        }
        update::spawn(&CONTROLLER_HWND, WM_APP_UPDATE);

        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                module,
                controller,
                tray_shown: false,
                tray_tip: String::new(),
                anim_on: false,
                card,
                card_frame: None,
                card_visible: false,
                hover_since: None,
                card_dismissed: false,
                can_reserve: reserve::supported(),
                shared,
                tx,
                snap: Snapshot::default(),
                cfg,
                widgets: Vec::new(),
                system_light: false,
                system_accent: 0xFF4C_C2FF,
                ticks: 0,
                seek_override: None,
                volume_flash: None,
                volume: None,
                mixer,
                mixer_open: false,
                popup: Popup::Mixer,
                outputs_rows: Vec::new(),
                outputs_current: String::new(),
                outputs_hover: None,
                outputs_frame: None,
                menu_rows: Vec::new(),
                menu_hover: None,
                menu_frame: None,
                mixer_rows: Vec::new(),
                mixer_hover: None,
                mixer_drag: None,
                mixer_frame: None,
                mixer_scale: 1.0,
                mixer_anchor: (0, 0, 0),
                mixer_was_active: false,
                mixer_away: None,
                notice: None,
                output: None,
            });
        });
        SetTimer(Some(controller), TIMER_ID, TICK_MS, None);
        with_app(|a| {
            a.sync_tray(false);
            a.tick();
        });

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

        with_app(|a| {
            if a.tray_shown {
                tray::remove(a.controller);
            }
        });
        if let Ok(main) = FindWindowW(w!("Shell_TrayWnd"), None) {
            reserve::restore(main);
        }
    }
}
