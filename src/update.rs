//! Checks GitHub for a newer release. Nothing is downloaded or installed;
//! the app only learns the latest version number and offers a link.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Foundation::Uri;
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASES_URL: &str = "https://github.com/austin-72/taskbar-media/releases/latest";
const API_URL: &str = "https://api.github.com/repos/austin-72/taskbar-media/releases/latest";

/// Set from the settings window.
pub static ENABLED: AtomicBool = AtomicBool::new(true);
static AVAILABLE: Mutex<Option<String>> = Mutex::new(None);

/// The newer version, if one has been found.
pub fn available() -> Option<String> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    AVAILABLE.lock().ok().and_then(|v| v.clone())
}

/// Check shortly after start and then every six hours.
pub fn spawn(hwnd: &'static AtomicIsize, notify_msg: u32) {
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        std::thread::sleep(Duration::from_secs(20));
        loop {
            if ENABLED.load(Ordering::Relaxed) {
                if let Some(latest) = latest_version() {
                    let newer = is_newer(&latest, VERSION).then_some(latest);
                    let changed = AVAILABLE.lock().map(|mut v| {
                        let changed = *v != newer;
                        *v = newer;
                        changed
                    });
                    let raw = hwnd.load(Ordering::Relaxed);
                    if matches!(changed, Ok(true)) && raw != 0 {
                        unsafe {
                            let _ = PostMessageW(Some(HWND(raw as *mut _)), notify_msg, WPARAM(0), LPARAM(0));
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(6 * 60 * 60));
        }
    });
}

/// The tag of the latest published (non-pre-release) release, without "v".
fn latest_version() -> Option<String> {
    let client = HttpClient::new().ok()?;
    // GitHub's API rejects requests without a User-Agent.
    let agent = HSTRING::from(format!("TaskbarMedia/{VERSION}"));
    client.DefaultRequestHeaders().ok()?.UserAgent().ok()?.TryParseAdd(&agent).ok()?;
    let uri = Uri::CreateUri(&HSTRING::from(API_URL)).ok()?;
    let body = client.GetStringAsync(&uri).ok()?.join().ok()?.to_string();

    let rest = &body[body.find("\"tag_name\"")? + 10..];
    let rest = &rest[rest.find('"')? + 1..];
    let tag = &rest[..rest.find('"')?];
    let version = tag.trim_start_matches(['v', 'V']);
    (!version.is_empty() && version.len() < 32).then(|| version.to_string())
}

fn parse(version: &str) -> Vec<u32> {
    version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn is_newer(candidate: &str, current: &str) -> bool {
    let (mut a, mut b) = (parse(candidate), parse(current));
    a.resize(3, 0);
    b.resize(3, 0);
    a > b
}
