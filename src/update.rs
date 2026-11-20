//! Checks GitHub for a newer release. Nothing is downloaded or installed;
//! the app only learns the latest version number and offers a link.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::HSTRING;
use windows::Foundation::Uri;
use windows_future::AsyncStatus;
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

/// Sleep for `seconds`, but wake early when the check is switched on in
/// settings, so that turning it on is followed by a check soon after.
fn pause(seconds: u64) {
    let was_on = ENABLED.load(Ordering::Relaxed);
    let begun = Instant::now();
    while begun.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(5));
        if !was_on && ENABLED.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Check shortly after start and then every six hours. A check that fails
/// (no network yet just after sign-in, say) is tried again after 5 minutes,
/// then 30, before going back to the six-hour rhythm.
pub fn spawn(hwnd: &'static AtomicIsize, notify_msg: u32) {
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        std::thread::sleep(Duration::from_secs(20));
        let mut failures = 0u32;
        loop {
            let mut next = 6 * 60 * 60;
            if ENABLED.load(Ordering::Relaxed) {
                let found = latest_version();
                if found.is_none() {
                    failures += 1;
                    next = match failures {
                        1 => 5 * 60,
                        2 => 30 * 60,
                        _ => 6 * 60 * 60,
                    };
                } else {
                    failures = 0;
                }
                if let Some(latest) = found {
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
            pause(next);
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
    // Wait, but not for ever: a stalled connection must not end the checks.
    let request = client.GetStringAsync(&uri).ok()?;
    let begun = Instant::now();
    while request.Status().ok()? == AsyncStatus::Started {
        if begun.elapsed() > Duration::from_secs(30) {
            let _ = request.Cancel();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let body = request.GetResults().ok()?.to_string();
    tag_version(&body)
}

/// The version in a GitHub "release" JSON document: its tag without "v".
fn tag_version(body: &str) -> Option<String> {
    let rest = &body[body.find("\"tag_name\"")? + 10..];
    let rest = &rest[rest.find('"')? + 1..];
    let tag = &rest[..rest.find('"')?];
    let version = tag.trim_start_matches(['v', 'V']);
    let plausible = !version.is_empty()
        && version.len() < 32
        && version.starts_with(|c: char| c.is_ascii_digit())
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    plausible.then(|| version.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions() {
        assert!(is_newer("1.8.1", "1.8.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0", "1.99.99"));
        assert!(is_newer("2", "1.0.0"));
    }

    #[test]
    fn same_or_older_versions() {
        assert!(!is_newer("1.8.0", "1.8.0"));
        assert!(!is_newer("1.8", "1.8.0"));
        assert!(!is_newer("1.7.9", "1.8.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("garbage", "1.0.0"));
        assert!(!is_newer("", "1.0.0"));
    }

    #[test]
    fn suffixes_are_ignored() {
        assert!(!is_newer("1.8.0-beta", "1.8.0"));
        assert!(is_newer("1.9.0-rc1", "1.8.0"));
        assert_eq!(parse("1.2.3.4"), vec![1, 2, 3]);
    }

    #[test]
    fn reads_the_tag_from_the_release_json() {
        let body = r#"{"url":"x","tag_name": "v1.9.0","name":"1.9.0"}"#;
        assert_eq!(tag_version(body).as_deref(), Some("1.9.0"));
        assert_eq!(tag_version(r#"{"tag_name":"2.0.1"}"#).as_deref(), Some("2.0.1"));
        assert_eq!(tag_version(r#"{"message":"Not Found"}"#), None);
        assert_eq!(tag_version(r#"{"tag_name":""}"#), None);
        assert_eq!(tag_version(r#"{"tag_name":"<script>"}"#), None);
        assert_eq!(tag_version(r#"{"tag_name":"#), None);
    }
}
