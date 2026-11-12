//! A small log of crashes and restarts, at
//! `%LOCALAPPDATA%\TaskbarMedia\crash.log`, so that "it disappeared" can be
//! diagnosed afterwards.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetLocalTime;

/// The log is started afresh once it passes this size.
const MAX_BYTES: u64 = 200 * 1024;

/// The folder the log lives in.
pub fn log_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("TaskbarMedia"))
}

fn log_path() -> Option<PathBuf> {
    Some(log_dir()?.join("crash.log"))
}

/// Add a line to the log, with the time and the app version. Never fails
/// loudly: a problem writing the log must not cause another one.
pub fn write(message: &str) {
    let Some(path) = log_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    let t = unsafe { GetLocalTime() };
    let line = format!(
        "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}] v{}: {}\r\n",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        env!("CARGO_PKG_VERSION"),
        message
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Record crashes: Rust panics (with their message and source line) and
/// hardware exceptions such as access violations (with the address, as an
/// offset into the exe).
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let text = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_string());
        write(&format!("panic at {place}: {text}"));
    }));
    unsafe {
        SetUnhandledExceptionFilter(Some(on_exception));
    }
}

unsafe extern "system" fn on_exception(info: *const EXCEPTION_POINTERS) -> i32 {
    // Let the process end (which the supervisor notices) after logging.
    const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
    if let Some(record) = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
        let base = GetModuleHandleW(None).unwrap_or(HMODULE::default()).0 as usize;
        let address = record.ExceptionAddress as usize;
        write(&format!(
            "crash: exception {:#010x} at exe+{:#x}",
            record.ExceptionCode.0 as u32,
            address.wrapping_sub(base)
        ));
    }
    EXCEPTION_EXECUTE_HANDLER
}
