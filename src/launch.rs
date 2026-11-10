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

/// "Spotify.exe" matches spotify.exe; "Chrome" / "MSEdge" match their exe stem.
fn exe_matches(path_or_name: &str, app_id_lower: &str) -> bool {
    let p = Path::new(path_or_name);
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let stem = p.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    !name.is_empty() && (name == app_id_lower || stem == app_id_lower)
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
