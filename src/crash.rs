//! A small log of crashes and restarts, at
//! `%LOCALAPPDATA%\TaskbarMedia\crash.log`, so that "it disappeared" can be
//! diagnosed afterwards.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HMODULE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_ALWAYS,
};
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

/// A line of text built without allocating memory, for use while crashing.
struct Line {
    bytes: [u8; 320],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line { bytes: [0; 320], len: 0 }
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = self.bytes.len() - self.len;
        let n = s.len().min(room);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// "[date time] vX.Y.Z: " into `line`.
fn stamp(line: &mut Line) {
    let t = unsafe { GetLocalTime() };
    let _ = write!(
        line,
        "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}] v{}: ",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        env!("CARGO_PKG_VERSION"),
    );
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
    let mut head = Line::new();
    stamp(&mut head);
    let line = format!("{}{}\r\n", String::from_utf8_lossy(&head.bytes[..head.len]), message);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// The log's path as a null-terminated wide string, worked out at start so
/// that the exception handler needs no memory, environment or std I/O.
static WIDE_PATH: OnceLock<Vec<u16>> = OnceLock::new();

/// Record crashes: Rust panics (with their message and source line) and
/// hardware exceptions such as access violations (with the address, as an
/// offset into the exe).
pub fn install() {
    if let Some(path) = log_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let _ = WIDE_PATH.set(wide);
    }
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

/// Runs in a process that is already broken (possibly in the middle of an
/// allocation), so it only formats into a buffer on the stack and makes
/// three plain system calls.
unsafe extern "system" fn on_exception(info: *const EXCEPTION_POINTERS) -> i32 {
    // Let the process end (which the supervisor notices) after logging.
    const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
    if let Some(record) = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
        let base = GetModuleHandleW(None).unwrap_or(HMODULE::default()).0 as usize;
        let address = record.ExceptionAddress as usize;
        let mut line = Line::new();
        stamp(&mut line);
        let _ = write!(
            line,
            "crash: exception {:#010x} at exe+{:#x}\r\n",
            record.ExceptionCode.0 as u32,
            address.wrapping_sub(base)
        );
        if let Some(path) = WIDE_PATH.get() {
            if let Ok(file) = CreateFileW(
                PCWSTR(path.as_ptr()),
                FILE_APPEND_DATA.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) {
                let _ = WriteFile(file, Some(&line.bytes[..line.len]), None, None);
                let _ = CloseHandle(file);
            }
        }
    }
    EXCEPTION_EXECUTE_HANDLER
}
