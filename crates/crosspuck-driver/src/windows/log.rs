use crosspuck_core::guest_driver::GuestLogLevel;
use crosspuck_core::protocol::{session_trace_label, SESSION_TRACE_ID_MASK};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;

static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static LOG_LEVEL: OnceLock<Mutex<GuestLogLevel>> = OnceLock::new();
static SESSION_TRACE_ID: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

pub fn set_log_level(level: GuestLogLevel) {
    let Ok(mut guard) = LOG_LEVEL
        .get_or_init(|| Mutex::new(crosspuck_core::guest_driver::config::DEFAULT_LOG_LEVEL))
        .lock()
    else {
        return;
    };
    *guard = level;
}

pub fn set_session_trace_id(session_trace_id: Option<u32>) {
    let Ok(mut guard) = SESSION_TRACE_ID.get_or_init(|| Mutex::new(None)).lock() else {
        return;
    };
    *guard = session_trace_id.map(|id| id & SESSION_TRACE_ID_MASK);
}

pub fn debug_line(message: &str) {
    write_line(GuestLogLevel::Debug, message);
}

pub fn error_line(message: &str) {
    write_line(GuestLogLevel::Error, message);
}

pub fn info_line(message: &str) {
    write_line(GuestLogLevel::Info, message);
}

pub fn trace_line(message: &str) {
    write_line(GuestLogLevel::Trace, message);
}

pub fn log_enabled(level: GuestLogLevel) -> bool {
    current_log_level().allows(level)
}

fn write_line(level: GuestLogLevel, message: &str) {
    if !current_log_level().allows(level) {
        return;
    }
    let message = decorate_message(message);
    append_file_line(&message);
    let Ok(message) = CString::new(message) else {
        return;
    };
    unsafe {
        OutputDebugStringA(message.as_ptr() as _);
    }
}

fn current_log_level() -> GuestLogLevel {
    LOG_LEVEL
        .get_or_init(|| Mutex::new(crosspuck_core::guest_driver::config::DEFAULT_LOG_LEVEL))
        .lock()
        .map(|guard| *guard)
        .unwrap_or(crosspuck_core::guest_driver::config::DEFAULT_LOG_LEVEL)
}

fn decorate_message(message: &str) -> String {
    let Some(session_trace_id) = current_session_trace_id() else {
        return message.to_string();
    };
    let label = session_trace_label(session_trace_id);
    if let Some(rest) = message.strip_prefix("[crosspuck]") {
        format!("[crosspuck:{label}]{rest}")
    } else {
        format!("[crosspuck:{label}] {message}")
    }
}

fn current_session_trace_id() -> Option<u32> {
    SESSION_TRACE_ID
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|guard| *guard)
}

fn append_file_line(message: &str) {
    let Ok(mut guard) = LOG_FILE.get_or_init(|| Mutex::new(open_log_file())).lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    let _ = writeln!(file, "{message}");
}

fn open_log_file() -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .ok()
}

fn log_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for ancestor in parent.ancestors() {
                if ancestor.join("steam.exe").exists() || ancestor.join("Steam.exe").exists() {
                    return ancestor.join("crosspuck-driver.log");
                }
            }
            return parent.join("crosspuck-driver.log");
        }
    }
    PathBuf::from("crosspuck-driver.log")
}
