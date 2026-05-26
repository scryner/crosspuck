use super::buffers::{
    input_slice, output_slice, report_id_from_buffer, zero_buffer, FALSE_U8, TRUE_U8,
};
use super::errors::{set_last_error_for, ERROR_DEVICE_NOT_CONNECTED_CODE};
use super::handles::{handle_for_profile, profile_for_handle, profile_for_open_handle};
use super::log::{debug_line, error_line, log_enabled, trace_line};
use super::proc::fn_from_mut;
use super::state;
use crosspuck_core::guest_driver::{
    path_may_be_virtual, GuestLogLevel, VirtualHandleId, VirtualHidProfile,
};
use std::ffi::c_void;
use std::slice;
use std::sync::{Mutex, OnceLock};
use windows_sys::core::{BOOL, PCSTR, PCWSTR};
use windows_sys::Win32::Foundation::{
    SetLastError, ERROR_INVALID_HANDLE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::IO::OVERLAPPED;

type CreateFileWFn = unsafe extern "system" fn(
    PCWSTR,
    u32,
    u32,
    *const SECURITY_ATTRIBUTES,
    u32,
    u32,
    HANDLE,
) -> HANDLE;
type CreateFileAFn = unsafe extern "system" fn(
    PCSTR,
    u32,
    u32,
    *const SECURITY_ATTRIBUTES,
    u32,
    u32,
    HANDLE,
) -> HANDLE;
type ReadFileFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut OVERLAPPED) -> BOOL;
type WriteFileFn =
    unsafe extern "system" fn(HANDLE, *const c_void, u32, *mut u32, *mut OVERLAPPED) -> BOOL;
type CloseHandleFn = unsafe extern "system" fn(HANDLE) -> BOOL;
type DeviceIoControlFn = unsafe extern "system" fn(
    HANDLE,
    u32,
    *mut c_void,
    u32,
    *mut c_void,
    u32,
    *mut u32,
    *mut OVERLAPPED,
) -> BOOL;

static ORIGINAL_CREATE_FILE_W: OnceLock<CreateFileWFn> = OnceLock::new();
static ORIGINAL_CREATE_FILE_A: OnceLock<CreateFileAFn> = OnceLock::new();
static ORIGINAL_READ_FILE: OnceLock<ReadFileFn> = OnceLock::new();
static ORIGINAL_WRITE_FILE: OnceLock<WriteFileFn> = OnceLock::new();
static ORIGINAL_CLOSE_HANDLE: OnceLock<CloseHandleFn> = OnceLock::new();
static ORIGINAL_DEVICE_IO_CONTROL: OnceLock<DeviceIoControlFn> = OnceLock::new();
static OPEN_SENTINELS: OnceLock<Mutex<Vec<(VirtualHidProfile, VirtualHandleId)>>> = OnceLock::new();
static REAL_HID_HANDLES: OnceLock<Mutex<Vec<(usize, String)>>> = OnceLock::new();

pub fn set_original_create_file_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_CREATE_FILE_W
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "CreateFileW trampoline already initialized".to_string())
}

pub fn set_original_create_file_a(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_CREATE_FILE_A
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "CreateFileA trampoline already initialized".to_string())
}

pub fn set_original_read_file(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_READ_FILE
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "ReadFile trampoline already initialized".to_string())
}

pub fn set_original_write_file(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_WRITE_FILE
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "WriteFile trampoline already initialized".to_string())
}

pub fn set_original_close_handle(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_CLOSE_HANDLE
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "CloseHandle trampoline already initialized".to_string())
}

pub fn set_original_device_io_control(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_DEVICE_IO_CONTROL
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "DeviceIoControl trampoline already initialized".to_string())
}

pub unsafe extern "system" fn detoured_create_file_w(
    file_name: PCWSTR,
    desired_access: u32,
    share_mode: u32,
    security_attributes: *const SECURITY_ATTRIBUTES,
    creation_disposition: u32,
    flags_and_attributes: u32,
    template_file: HANDLE,
) -> HANDLE {
    if let Some(path) = wide_z_to_string(file_name) {
        if let Some(handle) = claim_virtual_path(&path) {
            return handle;
        }
    }

    let handle = ORIGINAL_CREATE_FILE_W
        .get()
        .copied()
        .map(|original| {
            original(
                file_name,
                desired_access,
                share_mode,
                security_attributes,
                creation_disposition,
                flags_and_attributes,
                template_file,
            )
        })
        .unwrap_or(INVALID_HANDLE_VALUE);
    if let Some(path) = wide_z_to_string(file_name) {
        remember_real_hid_handle(handle, &path, "CreateFileW");
    }
    handle
}

pub unsafe extern "system" fn detoured_create_file_a(
    file_name: PCSTR,
    desired_access: u32,
    share_mode: u32,
    security_attributes: *const SECURITY_ATTRIBUTES,
    creation_disposition: u32,
    flags_and_attributes: u32,
    template_file: HANDLE,
) -> HANDLE {
    if let Some(path) = narrow_z_to_string(file_name) {
        if let Some(handle) = claim_virtual_path(&path) {
            return handle;
        }
    }

    let handle = ORIGINAL_CREATE_FILE_A
        .get()
        .copied()
        .map(|original| {
            original(
                file_name,
                desired_access,
                share_mode,
                security_attributes,
                creation_disposition,
                flags_and_attributes,
                template_file,
            )
        })
        .unwrap_or(INVALID_HANDLE_VALUE);
    if let Some(path) = narrow_z_to_string(file_name) {
        remember_real_hid_handle(handle, &path, "CreateFileA");
    }
    handle
}

pub unsafe extern "system" fn detoured_read_file(
    file: HANDLE,
    buffer: *mut c_void,
    bytes_to_read: u32,
    bytes_read: *mut u32,
    overlapped: *mut OVERLAPPED,
) -> BOOL {
    if profile_for_handle(file).is_some() && profile_for_open_handle(file).is_none() {
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8.into();
    }
    if let Some(profile) = profile_for_open_handle(file) {
        let result = read_virtual_file(profile, buffer, bytes_to_read, bytes_read);
        return u8_to_bool(result);
    }

    ORIGINAL_READ_FILE
        .get()
        .copied()
        .map_or(FALSE_U8.into(), |original| {
            original(file, buffer, bytes_to_read, bytes_read, overlapped)
        })
}

pub unsafe extern "system" fn detoured_write_file(
    file: HANDLE,
    buffer: *const c_void,
    bytes_to_write: u32,
    bytes_written: *mut u32,
    overlapped: *mut OVERLAPPED,
) -> BOOL {
    if profile_for_handle(file).is_some() && profile_for_open_handle(file).is_none() {
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8.into();
    }
    if let Some(profile) = profile_for_open_handle(file) {
        let result = write_virtual_file(profile, buffer, bytes_to_write, bytes_written);
        return u8_to_bool(result);
    }

    let real_hid_path = real_hid_path_for_handle(file);
    let result = ORIGINAL_WRITE_FILE
        .get()
        .copied()
        .map_or(FALSE_U8.into(), |original| {
            original(file, buffer, bytes_to_write, bytes_written, overlapped)
        });
    if let Some(path) = real_hid_path.filter(|_| log_enabled(GuestLogLevel::Trace)) {
        let written = if !bytes_written.is_null() {
            *bytes_written
        } else {
            0
        };
        trace_line(&format!(
            "[crosspuck] WriteFile passthrough real_hid handle={file:p} requested={bytes_to_write} written={written} overlapped={} result={} path={} payload={}",
            !overlapped.is_null(),
            result != 0,
            summarize_hid_path(&path),
            payload_preview(buffer, bytes_to_write)
        ));
    }
    result
}

pub unsafe extern "system" fn detoured_close_handle(handle: HANDLE) -> BOOL {
    if let Some(profile) = profile_for_handle(handle) {
        if close_virtual_handle(profile) {
            return TRUE_U8.into();
        }
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8.into();
    }

    let real_hid_path = forget_real_hid_handle(handle);
    let result = ORIGINAL_CLOSE_HANDLE
        .get()
        .copied()
        .map_or(FALSE_U8.into(), |original| original(handle));
    if let Some(path) = real_hid_path.filter(|_| log_enabled(GuestLogLevel::Trace)) {
        trace_line(&format!(
            "[crosspuck] CloseHandle passthrough real_hid handle={handle:p} closed={} path={}",
            result != 0,
            summarize_hid_path(&path)
        ));
    }
    result
}

pub unsafe extern "system" fn detoured_device_io_control(
    device: HANDLE,
    io_control_code: u32,
    in_buffer: *mut c_void,
    in_buffer_size: u32,
    out_buffer: *mut c_void,
    out_buffer_size: u32,
    bytes_returned: *mut u32,
    overlapped: *mut OVERLAPPED,
) -> BOOL {
    if profile_for_handle(device).is_some() && profile_for_open_handle(device).is_none() {
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8.into();
    }
    if profile_for_open_handle(device).is_some() {
        trace_virtual_ioctl(
            io_control_code,
            in_buffer as *const c_void,
            in_buffer_size,
            out_buffer_size,
        );
        let report_id = report_id_from_buffer(in_buffer as *const c_void, in_buffer_size);
        zero_buffer(out_buffer, out_buffer_size);
        if !out_buffer.is_null() && out_buffer_size > 0 {
            *(out_buffer as *mut u8) = report_id;
        }
        if !bytes_returned.is_null() {
            *bytes_returned = out_buffer_size;
        }
        return TRUE_U8.into();
    }

    let real_hid_path = real_hid_path_for_handle(device);
    let result = ORIGINAL_DEVICE_IO_CONTROL
        .get()
        .copied()
        .map_or(FALSE_U8.into(), |original| {
            original(
                device,
                io_control_code,
                in_buffer,
                in_buffer_size,
                out_buffer,
                out_buffer_size,
                bytes_returned,
                overlapped,
            )
        });
    if let Some(path) = real_hid_path.filter(|_| log_enabled(GuestLogLevel::Trace)) {
        let returned = if !bytes_returned.is_null() {
            *bytes_returned
        } else {
            0
        };
        trace_line(&format!(
            "[crosspuck] DeviceIoControl passthrough real_hid handle={device:p} code=0x{io_control_code:08X} in={in_buffer_size} out={out_buffer_size} returned={returned} overlapped={} result={} path={} payload={}",
            !overlapped.is_null(),
            result != 0,
            summarize_hid_path(&path),
            payload_preview(in_buffer as *const c_void, in_buffer_size)
        ));
    }
    result
}

unsafe fn claim_virtual_path(path: &str) -> Option<HANDLE> {
    if !path_may_be_virtual(path) {
        return None;
    }

    let Some(runtime) = state::runtime() else {
        debug_line(&format!(
            "[crosspuck] CreateFile path={path:?} not claimed: runtime missing"
        ));
        return None;
    };
    let Some(catalog) = state::catalog_if_connected("CreateFile") else {
        debug_line(&format!(
            "[crosspuck] CreateFile path={path:?} not claimed: cached catalog unavailable"
        ));
        return None;
    };
    let Some(profile) = catalog.profile_for_path(path) else {
        debug_line(&format!(
            "[crosspuck] CreateFile path={path:?} not claimed: no virtual profile"
        ));
        return None;
    };
    match runtime.open_profile(profile) {
        Ok(handle_id) => {
            remember_virtual_handle(profile, handle_id);
            let handle = handle_for_profile(profile);
            debug_line(&format!(
                "[crosspuck] CreateFile virtual profile={} handle={handle:p} path={path:?}",
                profile.label()
            ));
            Some(handle)
        }
        Err(error) => {
            error_line(&format!(
                "[crosspuck] CreateFile virtual failed profile={} path={path:?} error={error}",
                profile.label()
            ));
            set_last_error_for(&error);
            None
        }
    }
}

unsafe fn read_virtual_file(
    profile: VirtualHidProfile,
    buffer: *mut c_void,
    bytes_to_read: u32,
    bytes_read: *mut u32,
) -> u8 {
    let Some(output) = output_slice(buffer, bytes_to_read) else {
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8;
    };
    let Some(runtime) = state::runtime() else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };

    match runtime.copy_next_input_report(profile, output) {
        Ok(Some(count)) => {
            trace_virtual_payload(profile, "ReadFile", &output[..count]);
            if !bytes_read.is_null() {
                *bytes_read = count as u32;
            }
            trace_line(&format!(
                "[crosspuck] ReadFile virtual profile={} requested={} returned={count}",
                profile.label(),
                bytes_to_read
            ));
            TRUE_U8
        }
        Ok(None) => {
            zero_buffer(buffer, bytes_to_read);
            if !bytes_read.is_null() {
                *bytes_read = 0;
            }
            TRUE_U8
        }
        Err(error) => {
            if !bytes_read.is_null() {
                *bytes_read = 0;
            }
            error_line(&format!(
                "[crosspuck] ReadFile virtual failed profile={} requested={} error={error}",
                profile.label(),
                bytes_to_read
            ));
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

unsafe fn write_virtual_file(
    profile: VirtualHidProfile,
    buffer: *const c_void,
    bytes_to_write: u32,
    bytes_written: *mut u32,
) -> u8 {
    let Some(payload) = input_slice(buffer, bytes_to_write) else {
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8;
    };
    let Some(runtime) = state::runtime() else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };

    match runtime.write_report(profile, payload) {
        Ok(count) => {
            trace_virtual_payload(profile, "WriteFile", payload);
            if !bytes_written.is_null() {
                *bytes_written = u32::from(count);
            }
            trace_line(&format!(
                "[crosspuck] WriteFile virtual profile={} requested={} accepted={count}",
                profile.label(),
                bytes_to_write
            ));
            TRUE_U8
        }
        Err(error) => {
            if !bytes_written.is_null() {
                *bytes_written = 0;
            }
            error_line(&format!(
                "[crosspuck] WriteFile virtual failed profile={} requested={} error={error}",
                profile.label(),
                bytes_to_write
            ));
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

fn remember_virtual_handle(profile: VirtualHidProfile, handle_id: VirtualHandleId) {
    if let Ok(mut handles) = OPEN_SENTINELS.get_or_init(|| Mutex::new(Vec::new())).lock() {
        handles.push((profile, handle_id));
    }
}

pub fn real_hid_path_for_handle(handle: HANDLE) -> Option<String> {
    let handle_value = handle as usize;
    REAL_HID_HANDLES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()?
        .iter()
        .find_map(|(stored, path)| (*stored == handle_value).then(|| path.clone()))
}

fn remember_real_hid_handle(handle: HANDLE, path: &str, source: &str) {
    if handle == INVALID_HANDLE_VALUE
        || !log_enabled(GuestLogLevel::Trace)
        || !path_looks_like_hid(path)
    {
        return;
    }
    let handle_value = handle as usize;
    let Ok(mut guard) = REAL_HID_HANDLES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    else {
        return;
    };
    if let Some((_, stored_path)) = guard.iter_mut().find(|(stored, _)| *stored == handle_value) {
        *stored_path = path.to_string();
    } else {
        guard.push((handle_value, path.to_string()));
    }
    trace_line(&format!(
        "[crosspuck] {source} passthrough real_hid handle={handle:p} path={}",
        summarize_hid_path(path)
    ));
}

fn forget_real_hid_handle(handle: HANDLE) -> Option<String> {
    let handle_value = handle as usize;
    let mut guard = REAL_HID_HANDLES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()?;
    let index = guard
        .iter()
        .position(|(stored, _)| *stored == handle_value)?;
    Some(guard.remove(index).1)
}

fn path_looks_like_hid(path: &str) -> bool {
    path.to_ascii_lowercase().contains("hid#")
}

pub fn summarize_hid_path(path: &str) -> String {
    let vid_pid = vid_pid_from_path(path)
        .map(|(vid, pid)| format!(" vid=0x{vid:04X} pid=0x{pid:04X}"))
        .unwrap_or_default();
    format!("{path:?}{vid_pid}")
}

fn vid_pid_from_path(path: &str) -> Option<(u16, u16)> {
    let lower = path.to_ascii_lowercase();
    let vid = parse_hex_after(&lower, "vid_")?;
    let pid = parse_hex_after(&lower, "pid_")?;
    Some((vid, pid))
}

fn parse_hex_after(value: &str, marker: &str) -> Option<u16> {
    let start = value.find(marker)? + marker.len();
    let hex = value.get(start..start + 4)?;
    u16::from_str_radix(hex, 16).ok()
}

unsafe fn payload_preview(buffer: *const c_void, len: u32) -> String {
    let Some(payload) = input_slice(buffer, len) else {
        return "<unavailable>".to_string();
    };
    let mut rendered = payload
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    if payload.len() > 32 {
        rendered.push_str(" ...");
    }
    rendered
}

fn close_virtual_handle(profile: VirtualHidProfile) -> bool {
    let Some(runtime) = state::runtime() else {
        return false;
    };
    let Some(handles) = OPEN_SENTINELS.get() else {
        return false;
    };
    let Ok(mut handles) = handles.lock() else {
        return false;
    };
    if let Some(index) = handles
        .iter()
        .rposition(|(stored_profile, _)| *stored_profile == profile)
    {
        let (_, handle_id) = handles.remove(index);
        let closed = runtime.close_handle(handle_id).is_ok();
        debug_line(&format!(
            "[crosspuck] CloseHandle virtual profile={} closed={closed}",
            profile.label()
        ));
        return closed;
    }
    debug_line(&format!(
        "[crosspuck] CloseHandle virtual profile={} closed=false",
        profile.label()
    ));
    false
}

unsafe fn wide_z_to_string(ptr: PCWSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0;
    while len < 32_768 && *ptr.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(slice::from_raw_parts(ptr, len)))
}

unsafe fn narrow_z_to_string(ptr: PCSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0;
    while len < 32_768 && *ptr.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf8_lossy(slice::from_raw_parts(ptr, len)).to_string())
}

fn u8_to_bool(value: u8) -> BOOL {
    i32::from(value)
}

unsafe fn trace_virtual_ioctl(
    io_control_code: u32,
    in_buffer: *const c_void,
    in_buffer_size: u32,
    out_buffer_size: u32,
) {
    let Some(payload) = input_slice(in_buffer, in_buffer_size) else {
        return;
    };
    let Some(runtime) = state::runtime() else {
        return;
    };
    if let Some(rendered) = runtime.trace_payload(payload) {
        trace_line(&format!(
            "[crosspuck] DeviceIoControl code=0x{io_control_code:08X} in={} out={} payload={rendered}",
            payload.len(),
            out_buffer_size
        ));
    } else {
        trace_line(&format!(
            "[crosspuck] DeviceIoControl code=0x{io_control_code:08X} in={} out={out_buffer_size}",
            payload.len()
        ));
    }
}

fn trace_virtual_payload(profile: VirtualHidProfile, operation: &str, payload: &[u8]) {
    let Some(runtime) = state::runtime() else {
        return;
    };
    if let Some(rendered) = runtime.trace_payload(payload) {
        trace_line(&format!(
            "[crosspuck] {operation} profile={} len={} payload={rendered}",
            profile.label(),
            payload.len()
        ));
    }
}
