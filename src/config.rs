//! User settings, stored in HKCU\Software\TaskbarMedia.

use std::ffi::c_void;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

const SETTINGS_KEY: PCWSTR = w!("Software\\TaskbarMedia");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("TaskbarMedia");
const THEME_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

pub const DISPLAY_MAIN: u32 = 0;
pub const DISPLAY_ALL: u32 = 1;
pub const DISPLAY_ONE: u32 = 2;

#[derive(Clone, PartialEq)]
pub struct Config {
    /// DISPLAY_MAIN, DISPLAY_ALL or DISPLAY_ONE.
    pub display_mode: u32,
    /// Monitor device name (for example `\\.\DISPLAY2`) when DISPLAY_ONE.
    pub display_name: String,
    /// 0 small, 1 medium, 2 large.
    pub text_size: u32,
    /// 0 compact, 1 normal, 2 wide.
    pub width: u32,
    /// Supersampled drawing (smooth) or hinted, pixel-snapped drawing (sharp).
    pub smooth: bool,
    /// 0 automatic (follow the Windows theme), 1 light text, 2 dark text.
    pub text_color: u32,
    pub show_times: bool,
    pub hide_paused: bool,
    pub spotify_only: bool,
    /// Scroll titles that are too long to fit, instead of cutting them off.
    pub scroll_titles: bool,
    /// Mouse wheel over the widget: 0 nothing, 1 system volume, 2 the
    /// player's own volume.
    pub wheel_mode: u32,
    /// Shuffle and repeat buttons, for players that support them.
    pub extra_buttons: bool,
    /// Take the seek bar's color from the album art.
    pub accent: bool,
    /// Clicking the title copies "Artist - Title".
    pub click_copy: bool,
    pub check_updates: bool,
    /// Keep clear of app buttons: reserve space (Windows 10) or shrink (11).
    pub avoid_apps: bool,
    pub tray_icon: bool,
    /// Sideways nudge in pixels, for the main taskbar and for the others.
    pub offset: i32,
    pub offset2: i32,
}

impl Config {
    pub fn load() -> Self {
        let d = |name: PCWSTR, default: u32, max: u32| {
            read_dword(SETTINGS_KEY, name).filter(|v| *v <= max).unwrap_or(default)
        };
        let off = |name: PCWSTR| {
            (read_dword(SETTINGS_KEY, name).unwrap_or(0) as i32).clamp(-4000, 4000)
        };
        Config {
            display_mode: d(w!("DisplayMode"), DISPLAY_MAIN, 2),
            display_name: read_string(SETTINGS_KEY, w!("DisplayName")).unwrap_or_default(),
            text_size: d(w!("TextSize"), 1, 2),
            width: d(w!("Width"), 1, 2),
            smooth: d(w!("Smooth"), 1, 1) == 1,
            text_color: d(w!("TextColor"), 0, 2),
            show_times: d(w!("ShowTimes"), 1, 1) == 1,
            hide_paused: d(w!("HidePaused"), 0, 1) == 1,
            spotify_only: d(w!("SpotifyOnly"), 0, 1) == 1,
            scroll_titles: d(w!("ScrollTitles"), 1, 1) == 1,
            // 1.0 stored on/off as WheelVolume; off stays off.
            wheel_mode: read_dword(SETTINGS_KEY, w!("WheelMode")).filter(|v| *v <= 2).unwrap_or(
                if read_dword(SETTINGS_KEY, w!("WheelVolume")) == Some(0) { 0 } else { 2 },
            ),
            extra_buttons: d(w!("ExtraButtons"), 1, 1) == 1,
            accent: d(w!("Accent"), 1, 1) == 1,
            click_copy: d(w!("ClickCopy"), 1, 1) == 1,
            check_updates: d(w!("CheckUpdates"), 1, 1) == 1,
            avoid_apps: d(w!("AvoidApps"), 1, 1) == 1,
            tray_icon: d(w!("TrayIcon"), 1, 1) == 1,
            offset: off(w!("Offset")),
            offset2: off(w!("Offset2")),
        }
    }

    pub fn save(&self) {
        write_dword(SETTINGS_KEY, w!("DisplayMode"), self.display_mode);
        write_string(SETTINGS_KEY, w!("DisplayName"), &self.display_name);
        write_dword(SETTINGS_KEY, w!("TextSize"), self.text_size);
        write_dword(SETTINGS_KEY, w!("Width"), self.width);
        write_dword(SETTINGS_KEY, w!("Smooth"), self.smooth as u32);
        write_dword(SETTINGS_KEY, w!("TextColor"), self.text_color);
        write_dword(SETTINGS_KEY, w!("ShowTimes"), self.show_times as u32);
        write_dword(SETTINGS_KEY, w!("HidePaused"), self.hide_paused as u32);
        write_dword(SETTINGS_KEY, w!("SpotifyOnly"), self.spotify_only as u32);
        write_dword(SETTINGS_KEY, w!("ScrollTitles"), self.scroll_titles as u32);
        write_dword(SETTINGS_KEY, w!("WheelMode"), self.wheel_mode);
        write_dword(SETTINGS_KEY, w!("ExtraButtons"), self.extra_buttons as u32);
        write_dword(SETTINGS_KEY, w!("Accent"), self.accent as u32);
        write_dword(SETTINGS_KEY, w!("ClickCopy"), self.click_copy as u32);
        write_dword(SETTINGS_KEY, w!("CheckUpdates"), self.check_updates as u32);
        write_dword(SETTINGS_KEY, w!("AvoidApps"), self.avoid_apps as u32);
        write_dword(SETTINGS_KEY, w!("TrayIcon"), self.tray_icon as u32);
        write_dword(SETTINGS_KEY, w!("Offset"), self.offset as u32);
        write_dword(SETTINGS_KEY, w!("Offset2"), self.offset2 as u32);
    }
}

/// Whether the taskbar uses the light Windows theme.
pub fn system_light_theme() -> bool {
    read_dword(THEME_KEY, w!("SystemUsesLightTheme")) == Some(1)
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

fn read_string(key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn write_string(key: PCWSTR, value: PCWSTR, data: &str) {
    let wide = HSTRING::from(data);
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            REG_SZ.0,
            Some(wide.as_ptr() as *const c_void),
            ((wide.len() + 1) * 2) as u32,
        );
    }
}

pub fn autostart_enabled() -> bool {
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size))
    };
    status == ERROR_SUCCESS
}

/// Adds or removes this exe (at its current path) from the per-user Run key.
pub fn set_autostart(enable: bool) {
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
