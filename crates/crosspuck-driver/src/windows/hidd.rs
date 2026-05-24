use super::buffers::{
    input_slice, output_slice, report_id_from_buffer, write_wide_string, zero_buffer, FALSE_U8,
    TRUE_U8,
};
use super::errors::{set_last_error_for, ERROR_DEVICE_NOT_CONNECTED_CODE};
use super::handles::{
    preparsed_for_profile, profile_for_handle, profile_for_open_handle, profile_for_preparsed,
};
use super::log::debug_line;
use super::real_hid;
use super::state;
use crosspuck_core::guest_driver::VirtualHidProfile;
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
    match open_virtual_profile(device) {
        Ok(Some(_)) => {}
        Ok(None) => return call_real_hidd_get_attributes(device, attributes),
        Err(result) => return result,
    }

    if attributes.is_null() {
        return FALSE_U8;
    }
    let Some(catalog) = state::runtime().and_then(|runtime| runtime.catalog()) else {
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
    TRUE_U8
}

#[no_mangle]
pub unsafe extern "system" fn HidD_FreePreparsedData(preparsed_data: *mut c_void) -> u8 {
    if profile_for_preparsed(preparsed_data).is_some() {
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
        Ok(Some(_)) => write_identity_string(buffer, buffer_len, |identity| &identity.manufacturer),
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
        Ok(Some(_)) => write_identity_string(buffer, buffer_len, |identity| &identity.product),
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
        Ok(Some(_)) => write_identity_string(buffer, buffer_len, |identity| &identity.serial),
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
        Ok(Some(_)) => match string_index {
            1 => write_identity_string(buffer, buffer_len, |identity| &identity.manufacturer),
            2 => write_identity_string(buffer, buffer_len, |identity| &identity.product),
            3 => write_identity_string(buffer, buffer_len, |identity| &identity.serial),
            _ => write_identity_string(buffer, buffer_len, |identity| &identity.product),
        },
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
            TRUE_U8
        }
        Ok(None) => {
            zero_buffer(report_buffer, report_buffer_len);
            TRUE_U8
        }
        Err(error) => {
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
            TRUE_U8
        }
        Err(error) => {
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
            return call_real_hidd_report(
                "HidD_SetFeature",
                device,
                report_buffer,
                report_buffer_len,
            )
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
            TRUE_U8
        }
        Err(error) => {
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
            return call_real_hidd_report(
                "HidD_SetOutputReport",
                device,
                report_buffer,
                report_buffer_len,
            )
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
            TRUE_U8
        }
        Err(error) => {
            set_last_error_for(&error);
            FALSE_U8
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_FlushQueue(device: HANDLE) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(_)) => TRUE_U8,
        Ok(None) => call_real_hidd_handle_bool("HidD_FlushQueue", device),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_SetNumInputBuffers(device: HANDLE, _count: u32) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(_)) => TRUE_U8,
        Ok(None) => call_real_hidd_set_num_input_buffers(device, _count),
        Err(result) => result,
    }
}

#[no_mangle]
pub unsafe extern "system" fn HidD_GetNumInputBuffers(device: HANDLE, count: *mut u32) -> u8 {
    match open_virtual_profile(device) {
        Ok(Some(_)) => {
            if count.is_null() {
                return FALSE_U8;
            }
            *count = 64;
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
    let Some(catalog) = state::runtime().and_then(|runtime| runtime.catalog()) else {
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
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, attributes))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_get_preparsed_data(
    device: HANDLE,
    preparsed_data: *mut *mut c_void,
) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut *mut c_void) -> u8;
    real_hid::resolve_proc("HidD_GetPreparsedData")
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, preparsed_data))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_free_preparsed_data(preparsed_data: *mut c_void) -> u8 {
    type RealFn = unsafe extern "system" fn(*mut c_void) -> u8;
    real_hid::resolve_proc("HidD_FreePreparsedData")
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(preparsed_data))
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
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, buffer, buffer_len))
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
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, string_index, buffer, buffer_len))
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
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, report_buffer, report_buffer_len))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_handle_bool(name: &str, device: HANDLE) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE) -> u8;
    real_hid::resolve_proc(name)
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_set_num_input_buffers(device: HANDLE, count: u32) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, u32) -> u8;
    real_hid::resolve_proc("HidD_SetNumInputBuffers")
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, count))
        .unwrap_or(FALSE_U8)
}

unsafe fn call_real_hidd_get_num_input_buffers(device: HANDLE, count: *mut u32) -> u8 {
    type RealFn = unsafe extern "system" fn(HANDLE, *mut u32) -> u8;
    real_hid::resolve_proc("HidD_GetNumInputBuffers")
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, count))
        .unwrap_or(FALSE_U8)
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
