use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;

static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

pub fn debug_line(message: &str) {
    append_file_line(message);
    let Ok(message) = CString::new(message) else {
        return;
    };
    unsafe {
        OutputDebugStringA(message.as_ptr() as _);
    }
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
    let path = std::env::var_os("CROSSPUCK_LOG_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("crosspuck-driver.log"));
    OpenOptions::new().create(true).append(true).open(path).ok()
}
