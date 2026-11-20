//! "Open the app that is playing": bring its window to the front, or launch /
//! re-activate it if it has no visible window (for example Spotify minimised
//! to the tray).

use std::path::Path;

use windows::core::{w, BOOL, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, LPARAM};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER,
    SW_RESTORE, SW_SHOWNORMAL, WS_EX_TOOLWINDOW,
};

pub fn open_app(app_id: &str) {
    if app_id.is_empty() {
        return;
    }
    unsafe {
        if let Some(hwnd) = find_window(app_id) {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
            return;
        }

        // No visible window. Unpackaged apps (Spotify from spotify.com reports
        // itself as "Spotify.exe") are re-activated by starting their exe
        // again; single-instance players just surface their existing window.
        if !app_id.contains('!') {
            if let Some(path) = running_process_path(app_id) {
                ShellExecuteW(None, w!("open"), &HSTRING::from(path), None, None, SW_SHOWNORMAL);
                return;
            }
        }

        // Store / packaged apps (and anything else with a Start menu entry)
        // can be activated by their AppUserModelID.
        let target = HSTRING::from(format!("shell:AppsFolder\\{app_id}"));
        ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL);
    }
}

struct Search {
    app_id: String,
    found: Option<HWND>,
}

unsafe fn find_window(app_id: &str) -> Option<HWND> {
    let mut search = Search { app_id: app_id.to_lowercase(), found: None };
    // EnumWindows walks top-level windows in Z order, so the first match is
    // the app's most recently used window.
    let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut Search as isize));
    search.found
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut Search);
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }
    if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
        return true.into();
    }
    if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
        return true.into();
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return true.into();
    }
    if window_matches(hwnd, &search.app_id) {
        search.found = Some(hwnd);
        return false.into();
    }
    true.into()
}

unsafe fn window_matches(hwnd: HWND, app_id: &str) -> bool {
    // 1. An AppUserModelID set on the window itself (UWP frames, Firefox...).
    if let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) {
        if let Ok(value) = store.GetValue(&PKEY_AppUserModel_ID) {
            let id = value.to_string();
            if !id.is_empty() && id.to_lowercase() == app_id {
                return true;
            }
        }
    }

    // 2. The owning process: its package identity, or its exe name.
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return false;
    }
    let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return false;
    };
    let mut matched = false;

    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    if GetApplicationUserModelId(process, &mut len, Some(PWSTR(buf.as_mut_ptr()))) == ERROR_SUCCESS {
        let id = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
        matched = id.to_lowercase() == app_id;
    }

    if !matched {
        let mut path = [0u16; 1024];
        let mut len = path.len() as u32;
        if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len)
            .is_ok()
        {
            let full = String::from_utf16_lossy(&path[..len as usize]);
            matched = exe_matches(&full, app_id);
        }
    }
    let _ = CloseHandle(process);
    matched
}

/// Whether process `pid` belongs to the app with this (lower-case) app id:
/// by package identity for Store apps, otherwise by exe name.
pub fn process_matches(pid: u32, app_id: &str) -> bool {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut matched = false;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        if GetApplicationUserModelId(process, &mut len, Some(PWSTR(buf.as_mut_ptr()))) == ERROR_SUCCESS {
            let id = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
            matched = id.to_lowercase() == app_id;
        }
        if !matched {
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len)
                .is_ok()
            {
                let full = String::from_utf16_lossy(&path[..len as usize]);
                // Sound is matched more loosely than windows are: a web
                // app's audio comes from its browser, and some players use
                // an id that is not their exe name at all.
                matched = exe_matches(&full, app_id)
                    || audio_exe(app_id).is_some_and(|exe| exe_matches(&full, &exe));
            }
        }
        let _ = CloseHandle(process);
        matched
    }
}

thread_local! {
    /// What `audio_exe` last worked out: (app id, exe stem or "", when).
    static AUDIO_EXE: std::cell::RefCell<Option<(String, String, std::time::Instant)>> =
        const { std::cell::RefCell::new(None) };
}

/// The exe (lower-case stem) whose sound belongs to a player whose id is
/// neither an exe name nor a package: a known alias, or else the exe of the
/// window that carries that id (how Firefox, with its per-install hashed id,
/// is found wherever it is installed). Looking through the windows is slow,
/// so the answer is remembered for a while.
fn audio_exe(app_id_lower: &str) -> Option<String> {
    if let Some(alias) = exe_alias(app_id_lower) {
        return Some(alias.to_string());
    }
    // Packaged apps are matched by package identity, and ids that are exe
    // names by name; neither needs this.
    if app_id_lower.contains('!') || app_id_lower.ends_with(".exe") {
        return None;
    }
    let cached = AUDIO_EXE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(id, _, at)| id == app_id_lower && at.elapsed() < std::time::Duration::from_secs(30))
            .map(|(_, exe, _)| exe.clone())
    });
    let exe = cached.unwrap_or_else(|| {
        let found = unsafe { find_window_by_id(app_id_lower) }
            .and_then(|hwnd| {
                let mut pid = 0u32;
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
                process_exe(pid)
            })
            .and_then(|name| Path::new(&name).file_stem().and_then(|s| s.to_str()).map(|s| s.to_lowercase()))
            .unwrap_or_default();
        AUDIO_EXE.with(|c| {
            *c.borrow_mut() = Some((app_id_lower.to_string(), found.clone(), std::time::Instant::now()));
        });
        found
    });
    (!exe.is_empty()).then_some(exe)
}

/// The first visible top-level window whose own AppUserModelID is `app_id`.
unsafe fn find_window_by_id(app_id_lower: &str) -> Option<HWND> {
    let mut search = Search { app_id: app_id_lower.to_string(), found: None };
    let _ = EnumWindows(Some(enum_by_id), LPARAM(&mut search as *mut Search as isize));
    search.found
}

unsafe extern "system" fn enum_by_id(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut Search);
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }
    if let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) {
        if let Ok(value) = store.GetValue(&PKEY_AppUserModel_ID) {
            if value.to_string().to_lowercase() == search.app_id {
                search.found = Some(hwnd);
                return false.into();
            }
        }
    }
    true.into()
}

/// The exe file name of a running process, for example "Spotify.exe".
pub fn process_exe(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = [0u16; 1024];
        let mut len = path.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&path[..len as usize]);
        Path::new(&full).file_name().and_then(|n| n.to_str()).map(|n| n.to_string())
    }
}

/// A readable name for an app id, for menus.
pub fn display_name(app_id: &str) -> String {
    let lower = app_id.to_lowercase();
    for (needle, name) in [
        ("spotify", "Spotify"),
        ("msedge", "Microsoft Edge"),
        ("chrome", "Google Chrome"),
        ("firefox", "Firefox"),
        ("308046b0af4a39cb", "Firefox"),
        ("e7cf176e110c211b", "Firefox"),
        ("6f193ccc56814779", "Firefox"),
        ("cfdb4b0b8dcd4e5c", "Firefox"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("vivaldi", "Vivaldi"),
        ("zunemusic", "Media Player"),
        ("zunevideo", "Movies & TV"),
        ("applemusic", "Apple Music"),
        ("itunes", "iTunes"),
        ("vlc", "VLC"),
        ("foobar2000", "foobar2000"),
        ("musicbee", "MusicBee"),
        ("tidal", "TIDAL"),
        ("amazon", "Amazon Music"),
    ] {
        if lower.contains(needle) {
            return name.to_string();
        }
    }
    // "Publisher.App_hash!Entry" -> "App"; "Some.exe" -> "Some".
    let base = app_id.split('!').next().unwrap_or(app_id);
    let base = base.split('_').next().unwrap_or(base);
    let base = base.rsplit('.').find(|part| !part.eq_ignore_ascii_case("exe")).unwrap_or(base);
    if base.is_empty() {
        app_id.to_string()
    } else {
        base.to_string()
    }
}

/// "Spotify.exe" matches spotify.exe; "Chrome" / "MSEdge" match their exe stem.
fn exe_matches(path_or_name: &str, app_id_lower: &str) -> bool {
    let p = Path::new(path_or_name);
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let stem = p.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    !name.is_empty() && (name == app_id_lower || stem == app_id_lower)
}

/// Players whose media-session id is neither their exe name nor a package
/// identity: the exe stem (lower case) their audio comes from. Only for
/// matching sound (see `audio_exe`); windows are matched by the id itself,
/// so that "open the player" brings up a web app and not its browser.
fn exe_alias(app_id_lower: &str) -> Option<&str> {
    // Firefox reports a hash of its install path (release, then other
    // channels); the usual ones are listed.
    const FIREFOX: [&str; 4] =
        ["308046b0af4a39cb", "e7cf176e110c211b", "6f193ccc56814779", "cfdb4b0b8dcd4e5c"];
    if FIREFOX.contains(&app_id_lower) || app_id_lower.starts_with("mozilla.firefox") {
        return Some("firefox");
    }
    // Installed web apps: "Chrome._crx_abc...", "MSEdge._crx_...", "Brave._crx_...".
    if let Some((browser, _)) = app_id_lower.split_once("._crx_") {
        return Some(browser);
    }
    None
}

unsafe fn running_process_path(app_id: &str) -> Option<String> {
    let app_id = app_id.to_lowercase();
    let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut result = None;
    let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
    while more {
        let n = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..n]);
        if exe_matches(&name, &app_id) {
            if let Ok(process) =
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID)
            {
                let mut path = [0u16; 1024];
                let mut len = path.len() as u32;
                if QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &mut len,
                )
                .is_ok()
                {
                    result = Some(String::from_utf16_lossy(&path[..len as usize]));
                }
                let _ = CloseHandle(process);
                if result.is_some() {
                    break;
                }
            }
        }
        more = Process32NextW(snapshot, &mut entry).is_ok();
    }
    let _ = CloseHandle(snapshot);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_match_app_ids() {
        assert!(exe_matches("Spotify.exe", "spotify.exe"));
        if cfg!(windows) {
            assert!(exe_matches("C:\\Users\\a\\AppData\\Roaming\\Spotify\\Spotify.exe", "spotify.exe"));
        }
        assert!(exe_matches("chrome.exe", "chrome"));
        assert!(!exe_matches("chrome.exe", "chrome._crx_abc"));
        assert!(!exe_matches("", "spotify.exe"));
        assert!(!exe_matches("notspotify.exe", "spotify.exe"));
    }

    #[test]
    fn aliases_are_for_sound_only() {
        assert_eq!(exe_alias("308046b0af4a39cb"), Some("firefox"));
        assert_eq!(exe_alias("chrome._crx_abcdef"), Some("chrome"));
        assert_eq!(exe_alias("msedge._crx_abcdef"), Some("msedge"));
        assert_eq!(exe_alias("spotify.exe"), None);
        assert_eq!(exe_alias("spotifyab.spotifymusic_zpdnekdrzrea0!spotify"), None);
    }

    #[test]
    fn readable_names() {
        assert_eq!(display_name("Spotify.exe"), "Spotify");
        assert_eq!(display_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(display_name("308046B0AF4A39CB"), "Firefox");
        assert_eq!(display_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
        assert_eq!(display_name("SomePlayer.exe"), "SomePlayer");
        assert_eq!(display_name("Acme.Tunes_abc123!App"), "Tunes");
    }
}
