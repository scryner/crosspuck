use super::handles::profile_for_preparsed;
use super::log::trace_line;
use super::proc::fn_from_const;
use super::real_hid;
use super::state;
use std::ffi::c_void;
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDP_BUTTON_CAPS, HIDP_LINK_COLLECTION_NODE, HIDP_VALUE_CAPS, USAGE_AND_PAGE,
};

const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;
const HIDP_STATUS_INVALID_PREPARSED_DATA: i32 = unchecked_status(0xC, 0x0001);
const HIDP_STATUS_INTERNAL_ERROR: i32 = unchecked_status(0xC, 0x0008);
const HIDP_STATUS_NOT_IMPLEMENTED: i32 = unchecked_status(0xC, 0x0020);

const fn unchecked_status(p1: u32, p2: u32) -> i32 {
    ((p1 << 28) | (0x11 << 16) | p2) as i32
}

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
        trace_line(&format!(
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
    trace_line(&format!(
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

#[no_mangle]
pub unsafe extern "system" fn HidP_GetButtonCaps(
    report_type: i32,
    button_caps: *mut c_void,
    button_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_GetButtonCaps virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        if !button_caps_length.is_null() {
            *button_caps_length = 0;
        }
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_button_caps(report_type, button_caps, button_caps_length, preparsed_data)
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetValueCaps(
    report_type: i32,
    value_caps: *mut c_void,
    value_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_GetValueCaps virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        if !value_caps_length.is_null() {
            *value_caps_length = 0;
        }
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_value_caps(report_type, value_caps, value_caps_length, preparsed_data)
}

#[no_mangle]
pub unsafe extern "system" fn HidP_MaxDataListLength(
    report_type: i32,
    preparsed_data: *mut c_void,
) -> u32 {
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_MaxDataListLength virtual profile={} report_type={report_type} returned=0",
            profile.label()
        ));
        return 0;
    }

    call_real_hidp_max_data_list_length(report_type, preparsed_data)
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetData(
    report_type: i32,
    data_list: *mut c_void,
    data_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut i8,
    report_length: u32,
) -> i32 {
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_GetData virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        if !data_length.is_null() {
            *data_length = 0;
        }
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }

    call_real_hidp_get_data(
        report_type,
        data_list,
        data_length,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetLinkCollectionNodes(
    link_collection_nodes: *mut HIDP_LINK_COLLECTION_NODE,
    link_collection_nodes_length: *mut u32,
    preparsed_data: *mut c_void,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !link_collection_nodes_length.is_null() {
            *link_collection_nodes_length = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_GetLinkCollectionNodes virtual profile={} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_link_collection_nodes(
        link_collection_nodes,
        link_collection_nodes_length,
        preparsed_data,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetSpecificButtonCaps(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    button_caps: *mut HIDP_BUTTON_CAPS,
    button_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !button_caps_length.is_null() {
            *button_caps_length = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_GetSpecificButtonCaps virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_specific_button_caps(
        report_type,
        usage_page,
        link_collection,
        usage,
        button_caps,
        button_caps_length,
        preparsed_data,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetSpecificValueCaps(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    value_caps: *mut HIDP_VALUE_CAPS,
    value_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !value_caps_length.is_null() {
            *value_caps_length = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_GetSpecificValueCaps virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_specific_value_caps(
        report_type,
        usage_page,
        link_collection,
        usage,
        value_caps,
        value_caps_length,
        preparsed_data,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetUsageValue(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_GetUsageValue virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }
    if usage_value.is_null() {
        return HIDP_STATUS_INTERNAL_ERROR;
    }

    call_real_hidp_get_usage_value(
        report_type,
        usage_page,
        link_collection,
        usage,
        usage_value,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetScaledUsageValue(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut i32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_GetScaledUsageValue virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }
    if usage_value.is_null() {
        return HIDP_STATUS_INTERNAL_ERROR;
    }

    call_real_hidp_get_scaled_usage_value(
        report_type,
        usage_page,
        link_collection,
        usage,
        usage_value,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetUsageValueArray(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut c_void,
    usage_value_byte_length: u16,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_GetUsageValueArray virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_usage_value_array(
        report_type,
        usage_page,
        link_collection,
        usage,
        usage_value,
        usage_value_byte_length,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetUsages(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage_list: *mut u16,
    usage_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !usage_length.is_null() {
            *usage_length = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_GetUsages virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_usages(
        report_type,
        usage_page,
        link_collection,
        usage_list,
        usage_length,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetUsagesEx(
    report_type: i32,
    link_collection: u16,
    button_list: *mut USAGE_AND_PAGE,
    usage_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !usage_length.is_null() {
            *usage_length = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_GetUsagesEx virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_usages_ex(
        report_type,
        link_collection,
        button_list,
        usage_length,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_GetExtendedAttributes(
    report_type: i32,
    data_index: u16,
    preparsed_data: *mut c_void,
    attributes: *mut c_void,
    length_attributes: *mut u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !length_attributes.is_null() {
            *length_attributes = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_GetExtendedAttributes virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_get_extended_attributes(
        report_type,
        data_index,
        preparsed_data,
        attributes,
        length_attributes,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_InitializeReportForID(
    report_type: i32,
    report_id: u8,
    preparsed_data: *mut c_void,
    report: *mut u8,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if profile_for_preparsed(preparsed_data).is_none() {
        return call_real_hidp_initialize_report_for_id(
            report_type,
            report_id,
            preparsed_data,
            report,
            report_length,
        );
    }
    if !report.is_null() && report_length > 0 {
        std::ptr::write_bytes(report, 0, report_length as usize);
        *report = report_id;
    }
    trace_line(&format!(
        "[crosspuck] HidP_InitializeReportForID report_type={report_type} report_id=0x{report_id:02X} preparsed={preparsed_data:p}"
    ));
    HIDP_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "system" fn HidP_SetData(
    report_type: i32,
    data_list: *mut c_void,
    data_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        if !data_length.is_null() {
            *data_length = 0;
        }
        trace_line(&format!(
            "[crosspuck] HidP_SetData virtual profile={} report_type={report_type} not implemented",
            profile.label()
        ));
        return HIDP_STATUS_NOT_IMPLEMENTED;
    }

    call_real_hidp_set_data(
        report_type,
        data_list,
        data_length,
        preparsed_data,
        report,
        report_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_MaxUsageListLength(
    report_type: i32,
    usage_page: u16,
    preparsed_data: *mut c_void,
) -> u32 {
    if preparsed_data.is_null() {
        return 0;
    }
    if let Some(profile) = profile_for_preparsed(preparsed_data) {
        trace_line(&format!(
            "[crosspuck] HidP_MaxUsageListLength virtual profile={} report_type={report_type} returned=0",
            profile.label()
        ));
        return 0;
    }

    call_real_hidp_max_usage_list_length(report_type, usage_page, preparsed_data)
}

#[no_mangle]
pub unsafe extern "system" fn HidP_SetUsageValue(
    report_type: i32,
    _usage_page: u16,
    _link_collection: u16,
    _usage: u16,
    _usage_value: u32,
    preparsed_data: *mut c_void,
    _report: *mut c_void,
    _report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if profile_for_preparsed(preparsed_data).is_none() {
        return call_real_hidp_set_usage_value(
            report_type,
            _usage_page,
            _link_collection,
            _usage,
            _usage_value,
            preparsed_data,
            _report,
            _report_length,
        );
    }
    trace_line(&format!(
        "[crosspuck] HidP_SetUsageValue unsupported report_type={report_type} preparsed={preparsed_data:p}"
    ));
    HIDP_STATUS_NOT_IMPLEMENTED
}

#[no_mangle]
pub unsafe extern "system" fn HidP_SetScaledUsageValue(
    report_type: i32,
    _usage_page: u16,
    _link_collection: u16,
    _usage: u16,
    _usage_value: i32,
    preparsed_data: *mut c_void,
    _report: *mut c_void,
    _report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if profile_for_preparsed(preparsed_data).is_none() {
        return call_real_hidp_set_scaled_usage_value(
            report_type,
            _usage_page,
            _link_collection,
            _usage,
            _usage_value,
            preparsed_data,
            _report,
            _report_length,
        );
    }
    trace_line(&format!(
        "[crosspuck] HidP_SetScaledUsageValue unsupported report_type={report_type} preparsed={preparsed_data:p}"
    ));
    HIDP_STATUS_NOT_IMPLEMENTED
}

#[no_mangle]
pub unsafe extern "system" fn HidP_SetUsageValueArray(
    report_type: i32,
    _usage_page: u16,
    _link_collection: u16,
    _usage: u16,
    _usage_value: *mut c_void,
    _usage_value_byte_length: u16,
    preparsed_data: *mut c_void,
    _report: *mut c_void,
    _report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if profile_for_preparsed(preparsed_data).is_none() {
        return call_real_hidp_set_usage_value_array(
            report_type,
            _usage_page,
            _link_collection,
            _usage,
            _usage_value,
            _usage_value_byte_length,
            preparsed_data,
            _report,
            _report_length,
        );
    }
    trace_line(&format!(
        "[crosspuck] HidP_SetUsageValueArray unsupported report_type={report_type} preparsed={preparsed_data:p}"
    ));
    HIDP_STATUS_NOT_IMPLEMENTED
}

#[no_mangle]
pub unsafe extern "system" fn HidP_SetUsages(
    report_type: i32,
    _usage_page: u16,
    _link_collection: u16,
    _usage_list: *mut u16,
    _usage_length: *mut u32,
    preparsed_data: *mut c_void,
    _report: *mut c_void,
    _report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if profile_for_preparsed(preparsed_data).is_none() {
        return call_real_hidp_set_usages(
            report_type,
            _usage_page,
            _link_collection,
            _usage_list,
            _usage_length,
            preparsed_data,
            _report,
            _report_length,
        );
    }
    trace_line(&format!(
        "[crosspuck] HidP_SetUsages unsupported report_type={report_type} preparsed={preparsed_data:p}"
    ));
    HIDP_STATUS_NOT_IMPLEMENTED
}

#[no_mangle]
pub unsafe extern "system" fn HidP_UnsetUsages(
    report_type: i32,
    _usage_page: u16,
    _link_collection: u16,
    _usage_list: *mut u16,
    _usage_length: *mut u32,
    preparsed_data: *mut c_void,
    _report: *mut c_void,
    _report_length: u32,
) -> i32 {
    if preparsed_data.is_null() {
        return HIDP_STATUS_INVALID_PREPARSED_DATA;
    }
    if profile_for_preparsed(preparsed_data).is_none() {
        return call_real_hidp_unset_usages(
            report_type,
            _usage_page,
            _link_collection,
            _usage_list,
            _usage_length,
            preparsed_data,
            _report,
            _report_length,
        );
    }
    trace_line(&format!(
        "[crosspuck] HidP_UnsetUsages unsupported report_type={report_type} preparsed={preparsed_data:p}"
    ));
    HIDP_STATUS_NOT_IMPLEMENTED
}

#[no_mangle]
pub unsafe extern "system" fn HidP_UsageListDifference(
    previous_usage_list: *mut u16,
    current_usage_list: *mut u16,
    break_usage_list: *mut u16,
    make_usage_list: *mut u16,
    usage_list_length: u32,
) -> i32 {
    call_real_hidp_usage_list_difference(
        previous_usage_list,
        current_usage_list,
        break_usage_list,
        make_usage_list,
        usage_list_length,
    )
}

#[no_mangle]
pub unsafe extern "system" fn HidP_TranslateUsagesToI8042ScanCodes(
    changed_usage_list: *mut u16,
    usage_list_length: u32,
    key_action: i32,
    modifier_state: *mut c_void,
    insert_codes_procedure: *mut c_void,
    insert_codes_context: *mut c_void,
) -> i32 {
    call_real_hidp_translate_usages_to_i8042_scan_codes(
        changed_usage_list,
        usage_list_length,
        key_action,
        modifier_state,
        insert_codes_procedure,
        insert_codes_context,
    )
}

unsafe fn call_real_hidp_get_caps(preparsed_data: *mut c_void, caps: *mut HidpCaps) -> i32 {
    type RealFn = unsafe extern "system" fn(*mut c_void, *mut HidpCaps) -> i32;
    real_hid::resolve_proc("HidP_GetCaps")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(preparsed_data, caps))
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_button_caps(
    report_type: i32,
    button_caps: *mut c_void,
    button_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    type RealFn = unsafe extern "system" fn(i32, *mut c_void, *mut u16, *mut c_void) -> i32;
    real_hid::resolve_proc("HidP_GetButtonCaps")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                button_caps,
                button_caps_length,
                preparsed_data,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_value_caps(
    report_type: i32,
    value_caps: *mut c_void,
    value_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    type RealFn = unsafe extern "system" fn(i32, *mut c_void, *mut u16, *mut c_void) -> i32;
    real_hid::resolve_proc("HidP_GetValueCaps")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(report_type, value_caps, value_caps_length, preparsed_data)
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_max_data_list_length(
    report_type: i32,
    preparsed_data: *mut c_void,
) -> u32 {
    type RealFn = unsafe extern "system" fn(i32, *mut c_void) -> u32;
    real_hid::resolve_proc("HidP_MaxDataListLength")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(report_type, preparsed_data))
        .unwrap_or(0)
}

unsafe fn call_real_hidp_get_data(
    report_type: i32,
    data_list: *mut c_void,
    data_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut i8,
    report_length: u32,
) -> i32 {
    type RealFn =
        unsafe extern "system" fn(i32, *mut c_void, *mut u32, *mut c_void, *mut i8, u32) -> i32;
    real_hid::resolve_proc("HidP_GetData")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                data_list,
                data_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_link_collection_nodes(
    link_collection_nodes: *mut HIDP_LINK_COLLECTION_NODE,
    link_collection_nodes_length: *mut u32,
    preparsed_data: *mut c_void,
) -> i32 {
    type RealFn =
        unsafe extern "system" fn(*mut HIDP_LINK_COLLECTION_NODE, *mut u32, *mut c_void) -> i32;
    real_hid::resolve_proc("HidP_GetLinkCollectionNodes")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                link_collection_nodes,
                link_collection_nodes_length,
                preparsed_data,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_specific_button_caps(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    button_caps: *mut HIDP_BUTTON_CAPS,
    button_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        u16,
        *mut HIDP_BUTTON_CAPS,
        *mut u16,
        *mut c_void,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetSpecificButtonCaps")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                button_caps,
                button_caps_length,
                preparsed_data,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_specific_value_caps(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    value_caps: *mut HIDP_VALUE_CAPS,
    value_caps_length: *mut u16,
    preparsed_data: *mut c_void,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        u16,
        *mut HIDP_VALUE_CAPS,
        *mut u16,
        *mut c_void,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetSpecificValueCaps")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                value_caps,
                value_caps_length,
                preparsed_data,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_usage_value(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        u16,
        *mut u32,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetUsageValue")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                usage_value,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_scaled_usage_value(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut i32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        u16,
        *mut i32,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetScaledUsageValue")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                usage_value,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_usage_value_array(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut c_void,
    usage_value_byte_length: u16,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        u16,
        *mut c_void,
        u16,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetUsageValueArray")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                usage_value,
                usage_value_byte_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_usages(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage_list: *mut u16,
    usage_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        *mut u16,
        *mut u32,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetUsages")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage_list,
                usage_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_usages_ex(
    report_type: i32,
    link_collection: u16,
    button_list: *mut USAGE_AND_PAGE,
    usage_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        *mut USAGE_AND_PAGE,
        *mut u32,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_GetUsagesEx")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                link_collection,
                button_list,
                usage_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_get_extended_attributes(
    report_type: i32,
    data_index: u16,
    preparsed_data: *mut c_void,
    attributes: *mut c_void,
    length_attributes: *mut u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(i32, u16, *mut c_void, *mut c_void, *mut u32) -> i32;
    real_hid::resolve_proc("HidP_GetExtendedAttributes")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                data_index,
                preparsed_data,
                attributes,
                length_attributes,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_initialize_report_for_id(
    report_type: i32,
    report_id: u8,
    preparsed_data: *mut c_void,
    report: *mut u8,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(i32, u8, *mut c_void, *mut u8, u32) -> i32;
    real_hid::resolve_proc("HidP_InitializeReportForID")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                report_id,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_set_data(
    report_type: i32,
    data_list: *mut c_void,
    data_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn =
        unsafe extern "system" fn(i32, *mut c_void, *mut u32, *mut c_void, *mut c_void, u32) -> i32;
    real_hid::resolve_proc("HidP_SetData")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                data_list,
                data_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_max_usage_list_length(
    report_type: i32,
    usage_page: u16,
    preparsed_data: *mut c_void,
) -> u32 {
    type RealFn = unsafe extern "system" fn(i32, u16, *mut c_void) -> u32;
    real_hid::resolve_proc("HidP_MaxUsageListLength")
        .map(|ptr| fn_from_const::<RealFn>(ptr)(report_type, usage_page, preparsed_data))
        .unwrap_or(0)
}

unsafe fn call_real_hidp_set_scaled_usage_value(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: i32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn =
        unsafe extern "system" fn(i32, u16, u16, u16, i32, *mut c_void, *mut c_void, u32) -> i32;
    real_hid::resolve_proc("HidP_SetScaledUsageValue")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                usage_value,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_set_usage_value(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn =
        unsafe extern "system" fn(i32, u16, u16, u16, u32, *mut c_void, *mut c_void, u32) -> i32;
    real_hid::resolve_proc("HidP_SetUsageValue")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                usage_value,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_set_usage_value_array(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage: u16,
    usage_value: *mut c_void,
    usage_value_byte_length: u16,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        u16,
        *mut c_void,
        u16,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_SetUsageValueArray")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage,
                usage_value,
                usage_value_byte_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_set_usages(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage_list: *mut u16,
    usage_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        *mut u16,
        *mut u32,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_SetUsages")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage_list,
                usage_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_unset_usages(
    report_type: i32,
    usage_page: u16,
    link_collection: u16,
    usage_list: *mut u16,
    usage_length: *mut u32,
    preparsed_data: *mut c_void,
    report: *mut c_void,
    report_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(
        i32,
        u16,
        u16,
        *mut u16,
        *mut u32,
        *mut c_void,
        *mut c_void,
        u32,
    ) -> i32;
    real_hid::resolve_proc("HidP_UnsetUsages")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                report_type,
                usage_page,
                link_collection,
                usage_list,
                usage_length,
                preparsed_data,
                report,
                report_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_usage_list_difference(
    previous_usage_list: *mut u16,
    current_usage_list: *mut u16,
    break_usage_list: *mut u16,
    make_usage_list: *mut u16,
    usage_list_length: u32,
) -> i32 {
    type RealFn = unsafe extern "system" fn(*mut u16, *mut u16, *mut u16, *mut u16, u32) -> i32;
    real_hid::resolve_proc("HidP_UsageListDifference")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                previous_usage_list,
                current_usage_list,
                break_usage_list,
                make_usage_list,
                usage_list_length,
            )
        })
        .unwrap_or(-1)
}

unsafe fn call_real_hidp_translate_usages_to_i8042_scan_codes(
    changed_usage_list: *mut u16,
    usage_list_length: u32,
    key_action: i32,
    modifier_state: *mut c_void,
    insert_codes_procedure: *mut c_void,
    insert_codes_context: *mut c_void,
) -> i32 {
    type RealFn =
        unsafe extern "system" fn(*mut u16, u32, i32, *mut c_void, *mut c_void, *mut c_void) -> i32;
    real_hid::resolve_proc("HidP_TranslateUsagesToI8042ScanCodes")
        .map(|ptr| {
            fn_from_const::<RealFn>(ptr)(
                changed_usage_list,
                usage_list_length,
                key_action,
                modifier_state,
                insert_codes_procedure,
                insert_codes_context,
            )
        })
        .unwrap_or(-1)
}
