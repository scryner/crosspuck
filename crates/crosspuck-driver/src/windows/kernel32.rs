use super::buffers::{
    input_slice, output_slice, report_id_from_buffer, zero_buffer, FALSE_U8, TRUE_U8,
};
use super::errors::{set_last_error_for, ERROR_DEVICE_NOT_CONNECTED_CODE};
use super::handles::{handle_for_profile, profile_for_handle, profile_for_open_handle};
use super::log::debug_line;
use super::state;
use crosspuck_core::guest_driver::{VirtualHandleId, VirtualHidProfile};
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

pub fn set_original_create_file_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_CREATE_FILE_W
        .set(unsafe { std::mem::transmute(ptr) })
        .map_err(|_| "CreateFileW trampoline already initialized".to_string())
}

pub fn set_original_create_file_a(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_CREATE_FILE_A
        .set(unsafe { std::mem::transmute(ptr) })
        .map_err(|_| "CreateFileA trampoline already initialized".to_string())
}

pub fn set_original_read_file(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_READ_FILE
        .set(unsafe { std::mem::transmute(ptr) })
        .map_err(|_| "ReadFile trampoline already initialized".to_string())
}

pub fn set_original_write_file(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_WRITE_FILE
        .set(unsafe { std::mem::transmute(ptr) })
        .map_err(|_| "WriteFile trampoline already initialized".to_string())
}

pub fn set_original_close_handle(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_CLOSE_HANDLE
        .set(unsafe { std::mem::transmute(ptr) })
        .map_err(|_| "CloseHandle trampoline already initialized".to_string())
}

pub fn set_original_device_io_control(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_DEVICE_IO_CONTROL
        .set(unsafe { std::mem::transmute(ptr) })
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

    ORIGINAL_CREATE_FILE_W
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
        .unwrap_or(INVALID_HANDLE_VALUE)
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

    ORIGINAL_CREATE_FILE_A
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
        .unwrap_or(INVALID_HANDLE_VALUE)
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

    ORIGINAL_WRITE_FILE
        .get()
        .copied()
        .map_or(FALSE_U8.into(), |original| {
            original(file, buffer, bytes_to_write, bytes_written, overlapped)
        })
}

pub unsafe extern "system" fn detoured_close_handle(handle: HANDLE) -> BOOL {
    if let Some(profile) = profile_for_handle(handle) {
        if close_virtual_handle(profile) {
            return TRUE_U8.into();
        }
        SetLastError(ERROR_INVALID_HANDLE);
        return FALSE_U8.into();
    }

    ORIGINAL_CLOSE_HANDLE
        .get()
        .copied()
        .map_or(FALSE_U8.into(), |original| original(handle))
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

    ORIGINAL_DEVICE_IO_CONTROL
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
        })
}

unsafe fn claim_virtual_path(path: &str) -> Option<HANDLE> {
    let runtime = state::runtime()?;
    let catalog = runtime.catalog()?;
    let profile = catalog.profile_for_path(path)?;
    match runtime.open_profile(profile) {
        Ok(handle_id) => {
            remember_virtual_handle(profile, handle_id);
            Some(handle_for_profile(profile))
        }
        Err(error) => {
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
            TRUE_U8
        }
        Err(error) => {
            if !bytes_written.is_null() {
                *bytes_written = 0;
            }
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
        return runtime.close_handle(handle_id).is_ok();
    }
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
        debug_line(&format!(
            "[crosspuck] DeviceIoControl code=0x{io_control_code:08X} in={} out={} payload={rendered}",
            payload.len(),
            out_buffer_size
        ));
    }
}

fn trace_virtual_payload(profile: VirtualHidProfile, operation: &str, payload: &[u8]) {
    let Some(runtime) = state::runtime() else {
        return;
    };
    if let Some(rendered) = runtime.trace_payload(payload) {
        debug_line(&format!(
            "[crosspuck] {operation} profile={} len={} payload={rendered}",
            profile.label(),
            payload.len()
        ));
    }
}
