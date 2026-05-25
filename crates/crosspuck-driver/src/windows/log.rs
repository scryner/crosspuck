use crosspuck_core::protocol::{session_trace_label, SESSION_TRACE_ID_MASK};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;

static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static SESSION_TRACE_ID: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

pub fn set_session_trace_id(session_trace_id: Option<u32>) {
    let Ok(mut guard) = SESSION_TRACE_ID.get_or_init(|| Mutex::new(None)).lock() else {
        return;
    };
    *guard = session_trace_id.map(|id| id & SESSION_TRACE_ID_MASK);
}

pub fn debug_line(message: &str) {
    let message = decorate_message(message);
    append_file_line(&message);
    let Ok(message) = CString::new(message) else {
        return;
    };
    unsafe {
        OutputDebugStringA(message.as_ptr() as _);
    }
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
