use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

static REAL_HID_MODULE: OnceLock<usize> = OnceLock::new();

pub unsafe fn resolve_proc(name: &str) -> Option<*const c_void> {
    let module = *REAL_HID_MODULE.get_or_init(load_real_hid_module);
    if module == 0 {
        return None;
    }

    let mut proc_name = Vec::with_capacity(name.len() + 1);
    proc_name.extend_from_slice(name.as_bytes());
    proc_name.push(0);
    GetProcAddress(module as _, proc_name.as_ptr()).map(|proc| proc as *const c_void)
}

fn load_real_hid_module() -> usize {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let path = system_root.join("System32").join("hid.dll");
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe { LoadLibraryW(wide.as_ptr()) as usize }
}
