use super::buffers::{
    input_slice, output_slice, report_id_from_buffer, write_wide_string, zero_buffer, FALSE_U8,
    TRUE_U8,
};
use super::errors::{set_last_error_for, ERROR_DEVICE_NOT_CONNECTED_CODE};
use super::handles::{
    preparsed_for_profile, profile_for_handle, profile_for_open_handle, profile_for_preparsed,
};
use super::kernel32;
use super::log::{debug_line, error_line, log_enabled, trace_line};
use super::proc::fn_from_const;
use super::real_hid;
use super::state;
use crosspuck_core::guest_driver::{GuestLogLevel, VirtualHidProfile};
use crosspuck_core::protocol::IdentityPayload;
use std::ffi::c_void;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{SetLastError, ERROR_INVALID_HANDLE, HANDLE};

pub(crate) const HID_INTERFACE_GUID: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);

#[repr(C)]
pub struct HiddAttributes {
    size: u32,
    vendor_id: u16,
    product_id: u16,
    version_number: u16,
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetHidGuid(guid: *mut GUID) {
    if !guid.is_null() {
        *guid = HID_INTERFACE_GUID;
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetAttributes(
    device: HANDLE,
    attributes: *mut HiddAttributes,
) -> u8 {
    let profile = match open_virtual_profile(device) {
        Ok(Some(profile)) => profile,
        Ok(None) => return call_real_hidd_get_attributes(device, attributes),
        Err(result) => return result,
    };

    if attributes.is_null() {
        return FALSE_U8;
    }
    let Some(catalog) = state::catalog("HidD_GetAttributes") else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };
    let identity = catalog.identity();
    *attributes = HiddAttributes {
        size: std::mem::size_of::<HiddAttributes>() as u32,
        vendor_id: identity.vendor_id,
        product_id: identity.product_id,
        version_number: identity.version_number,
    };
    debug_line(&format!(
        "[crosspuck] HidD_GetAttributes virtual profile={} vid=0x{:04X} pid=0x{:04X} version=0x{:04X}",
        profile.label(),
        identity.vendor_id,
        identity.product_id,
        identity.version_number
    ));
    TRUE_U8
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetPreparsedData(
    device: HANDLE,
    preparsed_data: *mut *mut c_void,
) -> u8 {
    let profile = match open_virtual_profile(device) {
        Ok(Some(profile)) => profile,
        Ok(None) => return call_real_hidd_get_preparsed_data(device, preparsed_data),
        Err(result) => return result,
    };
    if preparsed_data.is_null() {
        return FALSE_U8;
    }
    *preparsed_data = preparsed_for_profile(profile);
    debug_line(&format!(
        "[crosspuck] HidD_GetPreparsedData virtual profile={} preparsed={:p}",
        profile.label(),
        *preparsed_data
    ));
    TRUE_U8
}

#[no_mangle]
pub unsafe extern "system" fn HidD_FreePreparsedData(preparsed_data: *mut c_void) -> u8 {
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        debug_line(&format!(
            "[crosspuck] HidD_FreePreparsedData virtual profile={} preparsed={preparsed_data:p}",
            profile.label()
        ));
        TRUE_U8
    } else {
        call_real_hidd_free_preparsed_data(preparsed_data)
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetManufacturerString(
    device: HANDLE,
    buffer: *mut c_void,
    buffer_len: u32,
) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            let result =
                write_identity_string(buffer, buffer_len, |identity| &identity.manufacturer);
            debug_line(&format!(
                "[crosspuck] HidD_GetManufacturerString virtual profile={} result={result}",
                profile.label()
            ));
            result
        }
        Ok(None) => call_real_hidd_string("HidD_GetManufacturerString", device, buffer, buffer_len),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetProductString(
    device: HANDLE,
    buffer: *mut c_void,
    buffer_len: u32,
) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            let result = write_identity_string(buffer, buffer_len, |identity| &identity.product);
            debug_line(&format!(
                "[crosspuck] HidD_GetProductString virtual profile={} result={result}",
                profile.label()
            ));
            result
        }
        Ok(None) => call_real_hidd_string("HidD_GetProductString", device, buffer, buffer_len),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetSerialNumberString(
    device: HANDLE,
    buffer: *mut c_void,
    buffer_len: u32,
) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            let result = write_identity_string(buffer, buffer_len, |identity| &identity.serial);
            debug_line(&format!(
                "[crosspuck] HidD_GetSerialNumberString virtual profile={} result={result}",
                profile.label()
            ));
            result
        }
        Ok(None) => call_real_hidd_string("HidD_GetSerialNumberString", device, buffer, buffer_len),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetIndexedString(
    device: HANDLE,
    string_index: u32,
    buffer: *mut c_void,
    buffer_len: u32,
) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            let result = match string_index {
                1 => write_identity_string(buffer, buffer_len, |identity| &identity.manufacturer),
                2 => write_identity_string(buffer, buffer_len, |identity| &identity.product),
                3 => write_identity_string(buffer, buffer_len, |identity| &identity.serial),
                _ => write_identity_string(buffer, buffer_len, |identity| &identity.product),
            };
            debug_line(&format!(
                "[crosspuck] HidD_GetIndexedString virtual profile={} index={} result={result}",
                profile.label(),
                string_index
            ));
            result
        }
        Ok(None) => call_real_hidd_indexed_string(device, string_index, buffer, buffer_len),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetInputReport(
    device: HANDLE,
    report_buffer: *mut c_void,
    report_buffer_len: u32,
) -> u8 {
    let profile = match open_virtual_profile(device) {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            return call_real_hidd_report(
                "HidD_GetInputReport",
                device,
                report_buffer,
                report_buffer_len,
            )
        }
        Err(result) => return result,
    };
    let Some(output) = output_slice(report_buffer, report_buffer_len) else {
        return FALSE_U8;
    };
    let Some(runtime) = state::runtime() else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };

    match runtime.copy_next_input_report(profile, output) {
        Ok(Some(count)) => {
            trace_virtual_payload(profile, "HidD_GetInputReport", &output[..count]);
            trace_line(&format!(
                "[crosspuck] HidD_GetInputReport virtual profile={} requested={} returned={count}",
                profile.label(),
                report_buffer_len
            ));
            TRUE_U8
        }
        Ok(None) => {
            zero_buffer(report_buffer, report_buffer_len);
            trace_line(&format!(
                "[crosspuck] HidD_GetInputReport virtual profile={} requested={} returned=0",
                profile.label(),
                report_buffer_len
            ));
            TRUE_U8
        }
        Err(error) => {
            error_line(&format!(
                "[crosspuck] HidD_GetInputReport virtual failed profile={} requested={} error={error}",
                profile.label(),
                report_buffer_len
            ));
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetFeature(
    device: HANDLE,
    report_buffer: *mut c_void,
    report_buffer_len: u32,
) -> u8 {
    let profile = match open_virtual_profile(device) {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            return call_real_hidd_report(
                "HidD_GetFeature",
                device,
                report_buffer,
                report_buffer_len,
            )
        }
        Err(result) => return result,
    };
    let report_id = report_id_from_buffer(report_buffer as *const c_void, report_buffer_len);
    let Some(output) = output_slice(report_buffer, report_buffer_len) else {
        return FALSE_U8;
    };
    let Some(runtime) = state::runtime() else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };

    match runtime.copy_feature_report(profile, report_id, output) {
        Ok(count) => {
            trace_virtual_payload(profile, "HidD_GetFeature", &output[..count]);
            trace_line(&format!(
                "[crosspuck] HidD_GetFeature virtual profile={} report_id=0x{report_id:02X} requested={} returned={count}",
                profile.label(),
                report_buffer_len
            ));
            TRUE_U8
        }
        Err(error) => {
            error_line(&format!(
                "[crosspuck] HidD_GetFeature virtual failed profile={} report_id=0x{report_id:02X} requested={} error={error}",
                profile.label(),
                report_buffer_len
            ));
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_SetFeature(
    device: HANDLE,
    report_buffer: *mut c_void,
    report_buffer_len: u32,
) -> u8 {
    let profile = match open_virtual_profile(device) {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            let result =
                call_real_hidd_report("HidD_SetFeature", device, report_buffer, report_buffer_len);
            log_real_hidd_output(
                "HidD_SetFeature",
                device,
                report_buffer,
                report_buffer_len,
                result,
            );
            return result;
        }
        Err(result) => return result,
    };
    let Some(payload) = input_slice(report_buffer as *const c_void, report_buffer_len) else {
        return FALSE_U8;
    };
    let Some(runtime) = state::runtime() else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };

    match runtime.set_feature(profile, payload) {
        Ok(_) => {
            trace_virtual_payload(profile, "HidD_SetFeature", payload);
            trace_line(&format!(
                "[crosspuck] HidD_SetFeature virtual profile={} len={}",
                profile.label(),
                payload.len()
            ));
            TRUE_U8
        }
        Err(error) => {
            error_line(&format!(
                "[crosspuck] HidD_SetFeature virtual failed profile={} len={} error={error}",
                profile.label(),
                payload.len()
            ));
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_SetOutputReport(
    device: HANDLE,
    report_buffer: *mut c_void,
    report_buffer_len: u32,
) -> u8 {
    let profile = match open_virtual_profile(device) {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            let result = call_real_hidd_report(
                "HidD_SetOutputReport",
                device,
                report_buffer,
                report_buffer_len,
            );
            log_real_hidd_output(
                "HidD_SetOutputReport",
                device,
                report_buffer,
                report_buffer_len,
                result,
            );
            return result;
        }
        Err(result) => return result,
    };
    let Some(payload) = input_slice(report_buffer as *const c_void, report_buffer_len) else {
        return FALSE_U8;
    };
    let Some(runtime) = state::runtime() else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };

    match runtime.set_output(profile, payload) {
        Ok(_) => {
            trace_virtual_payload(profile, "HidD_SetOutputReport", payload);
            trace_line(&format!(
                "[crosspuck] HidD_SetOutputReport virtual profile={} len={}",
                profile.label(),
                payload.len()
            ));
            TRUE_U8
        }
        Err(error) => {
            error_line(&format!(
                "[crosspuck] HidD_SetOutputReport virtual failed profile={} len={} error={error}",
                profile.label(),
                payload.len()
            ));
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_FlushQueue(device: HANDLE) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            debug_line(&format!(
                "[crosspuck] HidD_FlushQueue virtual profile={}",
                profile.label()
            ));
            TRUE_U8
        }
        Ok(None) => call_real_hidd_handle_bool("HidD_FlushQueue", device),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_SetNumInputBuffers(device: HANDLE, _count: u32) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            debug_line(&format!(
                "[crosspuck] HidD_SetNumInputBuffers virtual profile={} count={_count}",
                profile.label()
            ));
            TRUE_U8
        }
        Ok(None) => call_real_hidd_set_num_input_buffers(device, _count),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetNumInputBuffers(device: HANDLE, count: *mut u32) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(profile)) => {
            if count.is_null() {
                return FALSE_U8;
            }
            *count = 64;
            debug_line(&format!(
                "[crosspuck] HidD_GetNumInputBuffers virtual profile={} count=64",
                profile.label()
            ));
            TRUE_U8
        }
        Ok(None) => call_real_hidd_get_num_input_buffers(device, count),
        Err(result) => result,
    }
}

unsafe fn write_identity_string(
    buffer: *mut c_void,
    buffer_len: u32,
    select: impl FnOnce(&IdentityPayload) -> &str,
) -> u8 {
    let Some(catalog) = state::catalog("HidD identity string") else {
        SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
        return FALSE_U8;
    };
    write_wide_string(buffer, buffer_len, select(catalog.identity()))
}

unsafe fn open_virtual_profile(device: HANDLE) -> Result<Option<VirtualHidProfile>, u8> {
    if let Some(profile) = profile_for_open_handle(device) {
        return Ok(Some(profile));
    }
    if profile_for_handle(device).is_some() {
        SetLastError(ERROR_INVALID_HANDLE);
        return Err(FALSE_U8);
    }
    Ok(None)
}

unsafe fn call_real_hidd_get_attributes(device: HANDLE, attributes: *mut HiddAttributes) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut HiddAttributes) -> u8;
    real_hid::resolve_proc("HidD_GetAttributes")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, attributes))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_get_preparsed_data(
    device: HANDLE,
    preparsed_data: *mut *mut c_void,
) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut *mut c_void) -> u8;
    real_hid::resolve_proc("HidD_GetPreparsedData")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, preparsed_data))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_free_preparsed_data(preparsed_data: *mut c_void) -> u8 {
    type RealFn = unsafe extern "system" fn(*mut c_void) -> u8;
    real_hid::resolve_proc("HidD_FreePreparsedData")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(preparsed_data))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_string(
    name: &str,
    device: HANDLE,
    buffer: *mut c_void,
    buffer_len: u32,
) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut c_void, u32) -> u8;
    real_hid::resolve_proc(name)
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, buffer, buffer_len))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_indexed_string(
    device: HANDLE,
    string_index: u32,
    buffer: *mut c_void,
    buffer_len: u32,
) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, u32, *mut c_void, u32) -> u8;
    real_hid::resolve_proc("HidD_GetIndexedString")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, string_index, buffer, buffer_len))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_report(
    name: &str,
    device: HANDLE,
    report_buffer: *mut c_void,
    report_buffer_len: u32,
) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut c_void, u32) -> u8;
    real_hid::resolve_proc(name)
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, report_buffer, report_buffer_len))
        .unwrap_or(FALSE_U8)
}

unsafe fn log_real_hidd_output(
    operation: &str,
    device: HANDLE,
    report_buffer: *mut c_void,
    report_buffer_len: u32,
    result: u8,
) {
    let path = kernel32::real_hid_path_for_handle(device)
        .map(|path| kernel32::summarize_hid_path(&path))
        .unwrap_or_else(|| "<unknown>".to_string());
    if !log_enabled(GuestLogLevel::Trace) {
        return;
    }
    trace_line(&format!(
        "[crosspuck] {operation} passthrough real_hid handle={device:p} len={report_buffer_len} result={} path={path} payload={}",
        result != 0,
        payload_preview(report_buffer as *const c_void, report_buffer_len)
    ));
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

unsafe fn call_real_hidd_handle_bool(name: &str, device: HANDLE) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE) -> u8;
    real_hid::resolve_proc(name)
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_set_num_input_buffers(device: HANDLE, count: u32) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, u32) -> u8;
    real_hid::resolve_proc("HidD_SetNumInputBuffers")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, count))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_get_num_input_buffers(device: HANDLE, count: *mut u32) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut u32) -> u8;
    real_hid::resolve_proc("HidD_GetNumInputBuffers")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(device, count))
        .unwrap_or(FALSE_U8)
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
