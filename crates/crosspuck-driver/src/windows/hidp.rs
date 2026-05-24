use super::handles::profile_for_preparsed;
use super::log::debug_line;
use super::real_hid;
use super::state;
use std::ffi::c_void;

const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;

#[repr(C)]
pub struct HidpCaps {
    usage: u16,
    usage_page: u16,
    input_report_byte_length: u16,
    output_report_byte_length: u16,
    feature_report_byte_length: u16,
    reserved: [u16; 17],
    number_link_collection_nodes: u16,
    number_input_button_caps: u16,
    number_input_value_caps: u16,
    number_input_data_indices: u16,
    number_output_button_caps: u16,
    number_output_value_caps: u16,
    number_output_data_indices: u16,
    number_feature_button_caps: u16,
    number_feature_value_caps: u16,
    number_feature_data_indices: u16,
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetCaps(
    preparsed_data: *mut c_void,
    caps: *mut HidpCaps,
) -> i32 {
    let Some(profile) = profile_for_preparsed(preparsed_data) else {
        debug_line(&format!(
            "[crosspuck] HidP_GetCaps passthrough preparsed={preparsed_data:p}"
        ));
        return call_real_hidp_get_caps(preparsed_data, caps);
    };
    if caps.is_null() {
        return -1;
    }

    let Some(catalog) = state::catalog("HidP_GetCaps") else {
        return -1;
    };
    let Some(descriptor) = catalog.descriptor(profile) else {
        return -1;
    };
    let core_caps = descriptor.caps();

    *caps = HidpCaps {
        usage: core_caps.usage,
        usage_page: core_caps.usage_page,
        input_report_byte_length: core_caps.input_report_byte_length,
        output_report_byte_length: core_caps.output_report_byte_length,
        feature_report_byte_length: core_caps.feature_report_byte_length,
        reserved: [0; 17],
        number_link_collection_nodes: core_caps.number_link_collection_nodes,
        number_input_button_caps: 0,
        number_input_value_caps: core_caps.number_input_value_caps,
        number_input_data_indices: core_caps.number_input_value_caps,
        number_output_button_caps: 0,
        number_output_value_caps: core_caps.number_output_value_caps,
        number_output_data_indices: core_caps.number_output_value_caps,
        number_feature_button_caps: 0,
        number_feature_value_caps: core_caps.number_feature_value_caps,
        number_feature_data_indices: core_caps.number_feature_value_caps,
    };
    debug_line(&format!(
        "[crosspuck] HidP_GetCaps virtual profile={} usage_page=0x{:04X} usage=0x{:04X} in={} out={} feature={}",
        profile.label(),
        core_caps.usage_page,
        core_caps.usage,
        core_caps.input_report_byte_length,
        core_caps.output_report_byte_length,
        core_caps.feature_report_byte_length
    ));
    HIDP_STATUS_SUCCESS
}

unsafe fn call_real_hidp_get_caps(preparsed_data: *mut c_void, caps: *mut HidpCaps) -> i32 {
    type RealFn = unsafe extern "system" fn(*mut c_void, *mut HidpCaps) -> i32;
    real_hid::resolve_proc("HidP_GetCaps")
        .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(preparsed_data, caps))
        .unwrap_or(-1)
}
