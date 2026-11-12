//! User settings, stored in HKCU\Software\TaskbarMedia.

use std::ffi::c_void;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegDeleteTreeW, RegEnumValueW, RegGetValueW, RegOpenKeyExW,
    RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ,
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
    /// 0 automatic (follow the Windows theme), 1 light text, 2 dark text,
    /// 3 custom (`text_rgb`).
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
    /// The same for taskbars on other displays, used when `layout2_on`.
    pub layout2: Vec<(u8, bool)>,
    pub layout2_on: bool,
    /// Custom colors (0xAARRGGBB), used when `text_color` / `accent_mode`
    /// is 3.
    pub text_rgb: u32,
    pub accent_rgb: u32,
    /// Show a larger card with the track details while hovering.
    pub hover_card: bool,
    /// 0 small, 1 medium, 2 large.
    pub button_size: u32,
    /// 0 "Title · Artist" over the seek bar, 1 title over the seek bar,
    /// 2 "Artist · Title" over the seek bar, 3 title over artist,
    /// 4 title over album.
    pub text_layout: u32,
    /// Index into `FONTS`.
    pub font: u32,
    /// Highlight color: 0 from the album art, 1 the Windows accent color,
    /// 2 none (same as the text), 3 custom (`accent_rgb`).
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
    /// Extra space between buttons: 0 compact, 1 normal, 2 roomy, 3 wide,
    /// 4 extra wide.
    pub button_gap: u32,
    /// Shuffle above repeat in one column, when both are shown.
    pub stack_extras: bool,
    /// Leave out buttons that cannot be used right now (shuffle and repeat
    /// for players without them, the player arrows with one player) rather
    /// than showing them greyed out.
    pub hide_unusable: bool,
    /// What the volume button's icon reflects: 0 the player's own volume,
    /// 1 the system volume.
    pub volume_target: u32,
    /// Clicking the output button: 0 lists the devices, 1 switches between
    /// `output_a` and `output_b`.
    pub output_click: u32,
    /// Device ids of the two outputs to switch between.
    pub output_a: String,
    pub output_b: String,
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
pub const ITEM_OUTPUT: u8 = 9;
pub const ITEM_TITLE: u8 = 10;
pub const ITEM_SEEK: u8 = 11;
pub const ITEM_PLAYERS: u8 = 12;
pub const ITEM_VOLUME: u8 = 13;
/// How many items there are.
pub const ITEM_COUNT: usize = 13;

/// (item, name in the registry, name in the settings window), default order.
pub const ITEMS: [(u8, &str, &str); ITEM_COUNT] = [
    (ITEM_PLAYERS, "players", "Player switcher (up / down)"),
    (ITEM_GEAR, "gear", "Settings button"),
    (ITEM_ART, "art", "Album art"),
    (ITEM_SHUFFLE, "shuffle", "Shuffle"),
    (ITEM_PREV, "prev", "Previous"),
    (ITEM_PLAY, "play", "Play / pause"),
    (ITEM_NEXT, "next", "Next"),
    (ITEM_REPEAT, "repeat", "Repeat"),
    (ITEM_INFO, "info", "Title over seek bar"),
    (ITEM_VOLUME, "volume", "Volume button (opens the mixer)"),
    (ITEM_OUTPUT, "output", "Output device"),
    (ITEM_TITLE, "title", "Title on its own"),
    (ITEM_SEEK, "seek", "Seek bar on its own"),
];

/// Items that are hidden unless the user turns them on.
fn hidden_by_default(item: u8) -> bool {
    // Settings are always a right-click away, so the gear is opt-in.
    matches!(item, ITEM_GEAR | ITEM_OUTPUT | ITEM_TITLE | ITEM_SEEK)
}

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
            let extra = *item == ITEM_SHUFFLE || *item == ITEM_REPEAT;
            (*item, !hidden_by_default(*item) && (shuffle_and_repeat || !extra))
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
    // Layouts saved before the player switcher existed: it takes the
    // settings button's place, just left of the album art.
    if !layout.iter().any(|(i, _)| *i == ITEM_PLAYERS) {
        let at = layout.iter().position(|(i, _)| *i == ITEM_ART).unwrap_or(0);
        layout.insert(at, (ITEM_PLAYERS, true));
        if let Some(gear) = layout.iter_mut().find(|(i, _)| *i == ITEM_GEAR) {
            gear.1 = false;
        }
    }
    for (item, _, _) in ITEMS {
        if !layout.iter().any(|(i, _)| *i == item) {
            // Items added in a later version than the saved layout.
            layout.push((item, !hidden_by_default(item)));
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
    /// The shown items for a taskbar, left to right, zero-padded.
    pub fn order(&self, main_taskbar: bool) -> [u8; ITEM_COUNT] {
        let layout = if main_taskbar || !self.layout2_on { &self.layout } else { &self.layout2 };
        let mut order = [0u8; ITEM_COUNT];
        for (slot, (item, _)) in order.iter_mut().zip(layout.iter().filter(|(_, shown)| *shown)) {
            *slot = *item;
        }
        order
    }

    /// Replace the layout (the other taskbars' one if `other`) and the
    /// options that shape it with a ready-made arrangement: 1 default,
    /// 2 minimal, 3 compact, 4 everything, 5 classic.
    pub fn apply_preset(&mut self, preset: usize, other: bool) {
        // The given items shown in that order, everything else hidden.
        let showing = |shown: &[u8]| {
            let mut layout: Vec<(u8, bool)> = shown.iter().map(|item| (*item, true)).collect();
            layout.extend(ITEMS.iter().filter(|(item, _, _)| !shown.contains(item)).map(|(item, _, _)| (*item, false)));
            layout
        };
        let (layout, width, gap) = match preset {
            2 => (showing(&[ITEM_ART, ITEM_PLAY, ITEM_TITLE]), 0, 1),
            3 => (showing(&[ITEM_ART, ITEM_PREV, ITEM_PLAY, ITEM_NEXT, ITEM_INFO, ITEM_VOLUME]), 0, 0),
            4 => (
                showing(&[
                    ITEM_PLAYERS,
                    ITEM_ART,
                    ITEM_SHUFFLE,
                    ITEM_PREV,
                    ITEM_PLAY,
                    ITEM_NEXT,
                    ITEM_REPEAT,
                    ITEM_INFO,
                    ITEM_VOLUME,
                    ITEM_OUTPUT,
                    ITEM_GEAR,
                ]),
                1,
                1,
            ),
            5 => (showing(&[ITEM_GEAR, ITEM_ART, ITEM_PREV, ITEM_PLAY, ITEM_NEXT, ITEM_INFO]), 1, 1),
            _ => (default_layout(true), 1, 1),
        };
        if other {
            self.layout2 = layout;
        } else {
            self.layout = layout;
        }
        self.width = width;
        self.button_gap = gap;
        self.button_size = 1;
        self.stack_extras = false;
        self.text_layout = 0;
    }

    /// Whether an item is on any taskbar's widget.
    pub fn shows(&self, item: u8) -> bool {
        self.order(true).contains(&item) || self.order(false).contains(&item)
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
            text_color: d(w!("TextColor"), 0, 3),
            layout2: read_string(SETTINGS_KEY, w!("Layout2"))
                .map(|text| parse_layout(&text))
                .unwrap_or_else(|| default_layout(true)),
            layout2_on: d(w!("Layout2On"), 0, 1) == 1,
            text_rgb: read_dword(SETTINGS_KEY, w!("TextRgb")).unwrap_or(0x00FF_FFFF) | 0xFF00_0000,
            accent_rgb: read_dword(SETTINGS_KEY, w!("AccentRgb")).unwrap_or(0x004C_C2FF) | 0xFF00_0000,
            hover_card: d(w!("HoverCard"), 1, 1) == 1,
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
            accent_mode: read_dword(SETTINGS_KEY, w!("AccentMode")).filter(|v| *v <= 3).unwrap_or(
                if read_dword(SETTINGS_KEY, w!("Accent")) == Some(0) { 2 } else { 0 },
            ),
            art_shape: d(w!("ArtShape"), 0, 2),
            seek_size: d(w!("SeekSize"), 1, 2),
            background: d(w!("Background"), 0, 2),
            art_click: d(w!("ArtClick"), 0, 2),
            button_gap: d(w!("ButtonGap"), 1, 4),
            stack_extras: d(w!("StackExtras"), 0, 1) == 1,
            hide_unusable: d(w!("HideUnusable"), 0, 1) == 1,
            volume_target: d(w!("VolumeTarget"), 0, 1),
            output_click: d(w!("OutputClick"), 0, 1),
            output_a: read_string(SETTINGS_KEY, w!("OutputA")).unwrap_or_default(),
            output_b: read_string(SETTINGS_KEY, w!("OutputB")).unwrap_or_default(),
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
        write_string(SETTINGS_KEY, w!("Layout2"), &format_layout(&self.layout2));
        write_dword(SETTINGS_KEY, w!("Layout2On"), self.layout2_on as u32);
        write_dword(SETTINGS_KEY, w!("TextRgb"), self.text_rgb & 0x00FF_FFFF);
        write_dword(SETTINGS_KEY, w!("AccentRgb"), self.accent_rgb & 0x00FF_FFFF);
        write_dword(SETTINGS_KEY, w!("HoverCard"), self.hover_card as u32);
        write_dword(SETTINGS_KEY, w!("ButtonSize"), self.button_size);
        write_dword(SETTINGS_KEY, w!("TextLayout"), self.text_layout);
        write_dword(SETTINGS_KEY, w!("Font"), self.font);
        write_dword(SETTINGS_KEY, w!("AccentMode"), self.accent_mode);
        write_dword(SETTINGS_KEY, w!("ArtShape"), self.art_shape);
        write_dword(SETTINGS_KEY, w!("SeekSize"), self.seek_size);
        write_dword(SETTINGS_KEY, w!("Background"), self.background);
        write_dword(SETTINGS_KEY, w!("ArtClick"), self.art_click);
        write_dword(SETTINGS_KEY, w!("TitleClick"), self.title_click);
        write_dword(SETTINGS_KEY, w!("ButtonGap"), self.button_gap);
        write_dword(SETTINGS_KEY, w!("StackExtras"), self.stack_extras as u32);
        write_dword(SETTINGS_KEY, w!("HideUnusable"), self.hide_unusable as u32);
        write_dword(SETTINGS_KEY, w!("VolumeTarget"), self.volume_target);
        write_dword(SETTINGS_KEY, w!("OutputClick"), self.output_click);
        write_string(SETTINGS_KEY, w!("OutputA"), &self.output_a);
        write_string(SETTINGS_KEY, w!("OutputB"), &self.output_b);
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

/// Every saved setting as text, one "Name=dword:N" or "Name=str:text" per
/// line. Reads the registry directly, so new settings are covered
/// automatically.
pub fn export_text() -> String {
    let mut out = String::from("; Taskbar Media settings\r\n");
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, SETTINGS_KEY, None, KEY_READ, &mut key) != ERROR_SUCCESS {
            return out;
        }
        for index in 0..500 {
            let mut name = [0u16; 128];
            let mut name_len = name.len() as u32;
            let mut kind = 0u32;
            let mut data = [0u8; 2048];
            let mut data_len = data.len() as u32;
            let status = RegEnumValueW(
                key,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            );
            if status != ERROR_SUCCESS {
                break;
            }
            let name = String::from_utf16_lossy(&name[..name_len as usize]);
            let data = &data[..data_len as usize];
            if kind == REG_DWORD.0 && data.len() == 4 {
                let value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                out.push_str(&format!("{name}=dword:{value}\r\n"));
            } else if kind == REG_SZ.0 {
                let wide: Vec<u16> = data
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .take_while(|c| *c != 0)
                    .collect();
                out.push_str(&format!("{name}=str:{}\r\n", String::from_utf16_lossy(&wide)));
            }
        }
        let _ = RegCloseKey(key);
    }
    out
}

/// Replace the saved settings with those in `text` (as written by
/// `export_text`). Returns false, changing nothing, if no setting was found
/// in it. Values are range-checked later, when the settings are loaded.
pub fn import_text(text: &str) -> bool {
    let mut values: Vec<(String, Result<u32, String>)> = Vec::new();
    for line in text.lines().take(1000) {
        let line = line.trim();
        let Some((name, value)) = line.split_once('=') else { continue };
        let name = name.trim();
        if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        if let Some(number) = value.strip_prefix("dword:") {
            if let Ok(number) = number.trim().parse::<u32>() {
                values.push((name.to_string(), Ok(number)));
            }
        } else if let Some(text) = value.strip_prefix("str:") {
            if text.len() <= 512 {
                values.push((name.to_string(), Err(text.to_string())));
            }
        }
    }
    if values.is_empty() {
        return false;
    }
    reset_all();
    for (name, value) in values {
        let name = HSTRING::from(name);
        match value {
            Ok(number) => write_dword(SETTINGS_KEY, PCWSTR(name.as_ptr()), number),
            Err(text) => write_string(SETTINGS_KEY, PCWSTR(name.as_ptr()), &text),
        }
    }
    true
}

/// Forget every saved setting. ("Start with Windows" is stored elsewhere and
/// is not affected.)
pub fn reset_all() {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, SETTINGS_KEY);
    }
}
