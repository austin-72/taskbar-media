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
    /// Every widget item, left to right, with whether it is shown.
    pub layout: Vec<(u8, bool)>,
    /// 0 small, 1 medium, 2 large.
    pub button_size: u32,
    /// 0 "Title · Artist" over the seek bar, 1 title over the seek bar,
    /// 2 "Artist · Title" over the seek bar, 3 title over artist,
    /// 4 title over album.
    pub text_layout: u32,
    /// Index into `FONTS`.
    pub font: u32,
    /// Highlight color: 0 from the album art, 1 the Windows accent color,
    /// 2 none (same as the text).
    pub accent_mode: u32,
    /// 0 rounded, 1 square, 2 circle.
    pub art_shape: u32,
    /// 0 thin, 1 normal, 2 thick.
    pub seek_size: u32,
    /// 0 none, 1 subtle, 2 solid.
    pub background: u32,
    /// Clicking the album art: 0 opens the player, 1 play/pause, 2 nothing.
    pub art_click: u32,
    /// Clicking the title: 0 copies it, 1 opens the player, 2 nothing.
    pub title_click: u32,
    pub check_updates: bool,
    /// Keep clear of app buttons: reserve space (Windows 10) or shrink (11).
    pub avoid_apps: bool,
    pub tray_icon: bool,
    /// Sideways nudge in pixels, for the main taskbar and for the others.
    pub offset: i32,
    pub offset2: i32,
}

/// The widget's items. The numbers are stored in layouts; never renumber.
pub const ITEM_GEAR: u8 = 1;
pub const ITEM_ART: u8 = 2;
pub const ITEM_SHUFFLE: u8 = 3;
pub const ITEM_PREV: u8 = 4;
pub const ITEM_PLAY: u8 = 5;
pub const ITEM_NEXT: u8 = 6;
pub const ITEM_REPEAT: u8 = 7;
pub const ITEM_INFO: u8 = 8;

/// (item, name in the registry, name in the settings window), default order.
pub const ITEMS: [(u8, &str, &str); 8] = [
    (ITEM_GEAR, "gear", "Settings button"),
    (ITEM_ART, "art", "Album art"),
    (ITEM_SHUFFLE, "shuffle", "Shuffle"),
    (ITEM_PREV, "prev", "Previous"),
    (ITEM_PLAY, "play", "Play / pause"),
    (ITEM_NEXT, "next", "Next"),
    (ITEM_REPEAT, "repeat", "Repeat"),
    (ITEM_INFO, "info", "Title and seek bar"),
];

/// Font choices; the first is the default.
pub const FONTS: [&str; 7] =
    ["Segoe UI", "Bahnschrift", "Calibri", "Verdana", "Tahoma", "Georgia", "Consolas"];

pub fn item_label(item: u8) -> &'static str {
    ITEMS.iter().find(|(i, _, _)| *i == item).map(|(_, _, label)| *label).unwrap_or("")
}

pub fn default_layout(shuffle_and_repeat: bool) -> Vec<(u8, bool)> {
    ITEMS
        .iter()
        .map(|(item, _, _)| {
            (*item, shuffle_and_repeat || (*item != ITEM_SHUFFLE && *item != ITEM_REPEAT))
        })
        .collect()
}

/// "gear,art,-shuffle,..." (a leading '-' means hidden). Unknown names are
/// dropped and missing items are added at the end, so the result always has
/// every item exactly once.
fn parse_layout(text: &str) -> Vec<(u8, bool)> {
    let mut layout: Vec<(u8, bool)> = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        let (name, shown) = match part.strip_prefix('-') {
            Some(name) => (name, false),
            None => (part, true),
        };
        if let Some((item, _, _)) = ITEMS.iter().find(|(_, key, _)| *key == name) {
            if !layout.iter().any(|(i, _)| i == item) {
                layout.push((*item, shown));
            }
        }
    }
    for (item, _, _) in ITEMS {
        if !layout.iter().any(|(i, _)| *i == item) {
            layout.push((item, true));
        }
    }
    layout
}

fn format_layout(layout: &[(u8, bool)]) -> String {
    layout
        .iter()
        .filter_map(|(item, shown)| {
            let (_, key, _) = ITEMS.iter().find(|(i, _, _)| i == item)?;
            Some(if *shown { key.to_string() } else { format!("-{key}") })
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The Windows accent color as 0xAARRGGBB, if one is set.
pub fn system_accent() -> Option<u32> {
    // Stored as 0xAABBGGRR.
    let c = read_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))?;
    Some(0xFF00_0000 | ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF))
}

impl Config {
    /// The shown items, left to right, zero-padded.
    pub fn order(&self) -> [u8; 8] {
        let mut order = [0u8; 8];
        for (slot, (item, _)) in order.iter_mut().zip(self.layout.iter().filter(|(_, shown)| *shown)) {
            *slot = *item;
        }
        order
    }

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
            layout: match read_string(SETTINGS_KEY, w!("Layout")) {
                Some(text) => parse_layout(&text),
                // 1.1 had a single switch for the shuffle and repeat buttons.
                None => default_layout(read_dword(SETTINGS_KEY, w!("ExtraButtons")) != Some(0)),
            },
            button_size: d(w!("ButtonSize"), 1, 2),
            text_layout: d(w!("TextLayout"), 0, 4),
            font: d(w!("Font"), 0, FONTS.len() as u32 - 1),
            // 1.1 stored these three as on/off switches.
            accent_mode: read_dword(SETTINGS_KEY, w!("AccentMode")).filter(|v| *v <= 2).unwrap_or(
                if read_dword(SETTINGS_KEY, w!("Accent")) == Some(0) { 2 } else { 0 },
            ),
            art_shape: d(w!("ArtShape"), 0, 2),
            seek_size: d(w!("SeekSize"), 1, 2),
            background: d(w!("Background"), 0, 2),
            art_click: d(w!("ArtClick"), 0, 2),
            title_click: read_dword(SETTINGS_KEY, w!("TitleClick")).filter(|v| *v <= 2).unwrap_or(
                if read_dword(SETTINGS_KEY, w!("ClickCopy")) == Some(0) { 2 } else { 0 },
            ),
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
        write_string(SETTINGS_KEY, w!("Layout"), &format_layout(&self.layout));
        write_dword(SETTINGS_KEY, w!("ButtonSize"), self.button_size);
        write_dword(SETTINGS_KEY, w!("TextLayout"), self.text_layout);
        write_dword(SETTINGS_KEY, w!("Font"), self.font);
        write_dword(SETTINGS_KEY, w!("AccentMode"), self.accent_mode);
        write_dword(SETTINGS_KEY, w!("ArtShape"), self.art_shape);
        write_dword(SETTINGS_KEY, w!("SeekSize"), self.seek_size);
        write_dword(SETTINGS_KEY, w!("Background"), self.background);
        write_dword(SETTINGS_KEY, w!("ArtClick"), self.art_click);
        write_dword(SETTINGS_KEY, w!("TitleClick"), self.title_click);
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
