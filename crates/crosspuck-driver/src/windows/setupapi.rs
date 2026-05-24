use super::hidd::HID_INTERFACE_GUID;
use super::log::debug_line;
use super::state;
use crosspuck_core::guest_driver::{VirtualHidProfile, VirtualHidProfileCatalog};
use std::ffi::c_void;
use std::mem;
use std::sync::{Mutex, OnceLock};
use windows_sys::core::{BOOL, GUID, PCSTR, PCWSTR};
use windows_sys::Win32::Foundation::{
    SetLastError, DEVPROPKEY, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, HANDLE,
    INVALID_HANDLE_VALUE,
};

const SPINT_ACTIVE: u32 = 0x0000_0001;
const SYNTHETIC_DEVICE_INFO_SET: HANDLE = 0x4350_5543_5345_5450usize as HANDLE;
const VIRTUAL_INTERFACE_RESERVED_BASE: usize = 0x4350_5543_5349_0000;

const REG_SZ: u32 = 1;
const REG_MULTI_SZ: u32 = 7;
const DEVPROP_TYPE_STRING: u32 = 18;
const DEVPROP_TYPE_STRING_LIST: u32 = 8210;

const SPDRP_DEVICEDESC: u32 = 0x0000_0000;
const SPDRP_HARDWAREID: u32 = 0x0000_0001;
const SPDRP_COMPATIBLEIDS: u32 = 0x0000_0002;
const SPDRP_SERVICE: u32 = 0x0000_0004;
const SPDRP_CLASS: u32 = 0x0000_0007;
const SPDRP_MFG: u32 = 0x0000_000B;
const SPDRP_FRIENDLYNAME: u32 = 0x0000_000C;
const SPDRP_ENUMERATOR_NAME: u32 = 0x0000_0016;
const SPDRP_LOCATION_PATHS: u32 = 0x0000_0023;

const DEVPKEY_DEVICE_DEVICE_DESC: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 2,
};
const DEVPKEY_DEVICE_HARDWARE_IDS: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 3,
};
const DEVPKEY_DEVICE_COMPATIBLE_IDS: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 4,
};
const DEVPKEY_DEVICE_SERVICE: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 6,
};
const DEVPKEY_DEVICE_CLASS: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 9,
};
const DEVPKEY_DEVICE_MANUFACTURER: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 13,
};
const DEVPKEY_DEVICE_FRIENDLY_NAME: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};
const DEVPKEY_DEVICE_ENUMERATOR_NAME: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 24,
};
const DEVPKEY_DEVICE_LOCATION_PATHS: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 37,
};
const DEVPKEY_DEVICE_BUS_REPORTED_DEVICE_DESC: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x540b947e_8b40_45bc_a8a2_6a0b894cbda2),
    pid: 4,
};
const DEVPKEY_DEVICE_INSTANCE_ID: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x78c34fc8_104a_4aca_9ea4_524d52996e57),
    pid: 256,
};

#[repr(C)]
struct SpDevinfoData {
    cb_size: u32,
    class_guid: GUID,
    dev_inst: u32,
    reserved: usize,
}

#[repr(C)]
struct SpDeviceInterfaceData {
    cb_size: u32,
    interface_class_guid: GUID,
    flags: u32,
    reserved: usize,
}

struct RegistryValue {
    reg_type: u32,
    entries: Vec<String>,
}

struct DevicePropertyValue {
    prop_type: u32,
    entries: Vec<String>,
}

type SetupDiGetClassDevsWFn = unsafe extern "system" fn(*const GUID, PCWSTR, HANDLE, u32) -> HANDLE;
type SetupDiGetClassDevsAFn = unsafe extern "system" fn(*const GUID, PCSTR, HANDLE, u32) -> HANDLE;
type SetupDiEnumDeviceInterfacesFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, *const GUID, u32, *mut c_void) -> BOOL;
type SetupDiGetDeviceInterfaceDetailWFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, *mut c_void, u32, *mut u32, *mut c_void) -> BOOL;
type SetupDiGetDeviceInterfaceDetailAFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, *mut c_void, u32, *mut u32, *mut c_void) -> BOOL;
type SetupDiEnumDeviceInfoFn = unsafe extern "system" fn(HANDLE, u32, *mut c_void) -> BOOL;
type SetupDiGetDeviceRegistryPropertyWFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut u8, u32, *mut u32) -> BOOL;
type SetupDiGetDeviceRegistryPropertyAFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut u8, u32, *mut u32) -> BOOL;
type SetupDiGetDeviceInstanceIdWFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, *mut u16, u32, *mut u32) -> BOOL;
type SetupDiGetDeviceInstanceIdAFn =
    unsafe extern "system" fn(HANDLE, *mut c_void, *mut u8, u32, *mut u32) -> BOOL;
type SetupDiGetDevicePropertyWFn = unsafe extern "system" fn(
    HANDLE,
    *mut c_void,
    *const DEVPROPKEY,
    *mut u32,
    *mut u8,
    u32,
    *mut u32,
    u32,
) -> BOOL;

static ORIGINAL_SETUPDI_GET_CLASS_DEVS_W: OnceLock<SetupDiGetClassDevsWFn> = OnceLock::new();
static ORIGINAL_SETUPDI_GET_CLASS_DEVS_A: OnceLock<SetupDiGetClassDevsAFn> = OnceLock::new();
static ORIGINAL_SETUPDI_ENUM_DEVICE_INTERFACES: OnceLock<SetupDiEnumDeviceInterfacesFn> =
    OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_W: OnceLock<
    SetupDiGetDeviceInterfaceDetailWFn,
> = OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_A: OnceLock<
    SetupDiGetDeviceInterfaceDetailAFn,
> = OnceLock::new();
static ORIGINAL_SETUPDI_ENUM_DEVICE_INFO: OnceLock<SetupDiEnumDeviceInfoFn> = OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_W: OnceLock<
    SetupDiGetDeviceRegistryPropertyWFn,
> = OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_A: OnceLock<
    SetupDiGetDeviceRegistryPropertyAFn,
> = OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_W: OnceLock<SetupDiGetDeviceInstanceIdWFn> =
    OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_A: OnceLock<SetupDiGetDeviceInstanceIdAFn> =
    OnceLock::new();
static ORIGINAL_SETUPDI_GET_DEVICE_PROPERTY_W: OnceLock<SetupDiGetDevicePropertyWFn> =
    OnceLock::new();
static SYNTHETIC_ENUM_BASES: OnceLock<Mutex<Vec<(usize, u32)>>> = OnceLock::new();
static VIRTUAL_DEVINSTS: OnceLock<Mutex<Vec<(u32, VirtualHidProfile)>>> = OnceLock::new();

pub fn set_original_setupdi_get_class_devs_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_CLASS_DEVS_W
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetClassDevsW trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_class_devs_a(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_CLASS_DEVS_A
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetClassDevsA trampoline already initialized".to_string())
}

pub fn set_original_setupdi_enum_device_interfaces(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_ENUM_DEVICE_INTERFACES
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiEnumDeviceInterfaces trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_interface_detail_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_W
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDeviceInterfaceDetailW trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_interface_detail_a(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_A
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDeviceInterfaceDetailA trampoline already initialized".to_string())
}

pub fn set_original_setupdi_enum_device_info(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_ENUM_DEVICE_INFO
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiEnumDeviceInfo trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_registry_property_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_W
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDeviceRegistryPropertyW trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_registry_property_a(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_A
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDeviceRegistryPropertyA trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_instance_id_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_W
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDeviceInstanceIdW trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_instance_id_a(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_A
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDeviceInstanceIdA trampoline already initialized".to_string())
}

pub fn set_original_setupdi_get_device_property_w(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SETUPDI_GET_DEVICE_PROPERTY_W
        .set(unsafe { mem::transmute(ptr) })
        .map_err(|_| "SetupDiGetDevicePropertyW trampoline already initialized".to_string())
}

pub unsafe extern "system" fn detoured_setupdi_get_class_devs_w(
    class_guid: *const GUID,
    enumerator: PCWSTR,
    hwnd_parent: HANDLE,
    flags: u32,
) -> HANDLE {
    let original = ORIGINAL_SETUPDI_GET_CLASS_DEVS_W
        .get()
        .copied()
        .map(|original| original(class_guid, enumerator, hwnd_parent, flags));
    if original.is_some_and(|handle| handle != INVALID_HANDLE_VALUE) {
        return original.unwrap();
    }
    if is_hid_interface_guid(class_guid) && runtime_catalog().is_some() {
        debug_line(&format!(
            "[crosspuck] SetupDiGetClassDevsW synthetic flags=0x{flags:08X}"
        ));
        SYNTHETIC_DEVICE_INFO_SET
    } else {
        original.unwrap_or(INVALID_HANDLE_VALUE)
    }
}

pub unsafe extern "system" fn detoured_setupdi_get_class_devs_a(
    class_guid: *const GUID,
    enumerator: PCSTR,
    hwnd_parent: HANDLE,
    flags: u32,
) -> HANDLE {
    let original = ORIGINAL_SETUPDI_GET_CLASS_DEVS_A
        .get()
        .copied()
        .map(|original| original(class_guid, enumerator, hwnd_parent, flags));
    if original.is_some_and(|handle| handle != INVALID_HANDLE_VALUE) {
        return original.unwrap();
    }
    if is_hid_interface_guid(class_guid) && runtime_catalog().is_some() {
        debug_line(&format!(
            "[crosspuck] SetupDiGetClassDevsA synthetic flags=0x{flags:08X}"
        ));
        SYNTHETIC_DEVICE_INFO_SET
    } else {
        original.unwrap_or(INVALID_HANDLE_VALUE)
    }
}

pub unsafe extern "system" fn detoured_setupdi_enum_device_interfaces(
    device_info_set: HANDLE,
    device_info_data: *mut c_void,
    interface_class_guid: *const GUID,
    member_index: u32,
    device_interface_data: *mut c_void,
) -> BOOL {
    if let Some(original) = ORIGINAL_SETUPDI_ENUM_DEVICE_INTERFACES.get().copied() {
        let result = original(
            device_info_set,
            device_info_data,
            interface_class_guid,
            member_index,
            device_interface_data,
        );
        if result != 0 {
            return result;
        }
    }

    if !is_hid_interface_guid(interface_class_guid) {
        SetLastError(ERROR_NO_MORE_ITEMS);
        return 0;
    }
    let Some((profile, _catalog)) = synthetic_profile_for_member(device_info_set, member_index)
    else {
        SetLastError(ERROR_NO_MORE_ITEMS);
        return 0;
    };
    if write_synthetic_device_interface_data(device_interface_data, interface_class_guid, profile) {
        debug_line(&format!(
            "[crosspuck] SetupDiEnumDeviceInterfaces synthetic index={} profile={}",
            member_index,
            profile.label()
        ));
        1
    } else {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        0
    }
}

pub unsafe extern "system" fn detoured_setupdi_get_device_interface_detail_w(
    device_info_set: HANDLE,
    device_interface_data: *mut c_void,
    device_interface_detail_data: *mut c_void,
    device_interface_detail_data_size: u32,
    required_size: *mut u32,
    device_info_data: *mut c_void,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_interface_data(device_interface_data) {
        return synthesize_device_interface_detail_w(
            profile,
            device_interface_detail_data,
            device_interface_detail_data_size,
            required_size,
            device_info_data,
        );
    }
    ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_W
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_interface_data,
                device_interface_detail_data,
                device_interface_detail_data_size,
                required_size,
                device_info_data,
            )
        })
}

pub unsafe extern "system" fn detoured_setupdi_get_device_interface_detail_a(
    device_info_set: HANDLE,
    device_interface_data: *mut c_void,
    device_interface_detail_data: *mut c_void,
    device_interface_detail_data_size: u32,
    required_size: *mut u32,
    device_info_data: *mut c_void,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_interface_data(device_interface_data) {
        return synthesize_device_interface_detail_a(
            profile,
            device_interface_detail_data,
            device_interface_detail_data_size,
            required_size,
            device_info_data,
        );
    }
    ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_A
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_interface_data,
                device_interface_detail_data,
                device_interface_detail_data_size,
                required_size,
                device_info_data,
            )
        })
}

pub unsafe extern "system" fn detoured_setupdi_enum_device_info(
    device_info_set: HANDLE,
    member_index: u32,
    device_info_data: *mut c_void,
) -> BOOL {
    if let Some(original) = ORIGINAL_SETUPDI_ENUM_DEVICE_INFO.get().copied() {
        let result = original(device_info_set, member_index, device_info_data);
        if result != 0 {
            remember_virtual_device_info(device_info_data, "OriginalEnumDeviceInfo");
            return result;
        }
    }

    let Some((profile, _catalog)) = synthetic_profile_for_member(device_info_set, member_index)
    else {
        SetLastError(ERROR_NO_MORE_ITEMS);
        return 0;
    };
    write_synthetic_device_info_data(device_info_data, profile, "SyntheticEnumDeviceInfo");
    debug_line(&format!(
        "[crosspuck] SetupDiEnumDeviceInfo synthetic index={} profile={}",
        member_index,
        profile.label()
    ));
    1
}

pub unsafe extern "system" fn detoured_setupdi_get_device_registry_property_w(
    device_info_set: HANDLE,
    device_info_data: *mut c_void,
    property: u32,
    property_reg_data_type: *mut u32,
    property_buffer: *mut u8,
    property_buffer_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_info_data(device_info_data) {
        if let Some(value) = registry_property_value(profile, property) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceRegistryPropertyW virtual profile={} property=0x{property:08X}",
                profile.label()
            ));
            return write_registry_w(
                value,
                property_reg_data_type,
                property_buffer,
                property_buffer_size,
                required_size,
            );
        }
    }
    ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_W
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_info_data,
                property,
                property_reg_data_type,
                property_buffer,
                property_buffer_size,
                required_size,
            )
        })
}

pub unsafe extern "system" fn detoured_setupdi_get_device_registry_property_a(
    device_info_set: HANDLE,
    device_info_data: *mut c_void,
    property: u32,
    property_reg_data_type: *mut u32,
    property_buffer: *mut u8,
    property_buffer_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_info_data(device_info_data) {
        if let Some(value) = registry_property_value(profile, property) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceRegistryPropertyA virtual profile={} property=0x{property:08X}",
                profile.label()
            ));
            return write_registry_a(
                value,
                property_reg_data_type,
                property_buffer,
                property_buffer_size,
                required_size,
            );
        }
    }
    ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_A
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_info_data,
                property,
                property_reg_data_type,
                property_buffer,
                property_buffer_size,
                required_size,
            )
        })
}

pub unsafe extern "system" fn detoured_setupdi_get_device_instance_id_w(
    device_info_set: HANDLE,
    device_info_data: *mut c_void,
    device_instance_id: *mut u16,
    device_instance_id_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_info_data(device_info_data) {
        if let Some(value) =
            runtime_catalog().and_then(|catalog| catalog.device_instance_id(profile))
        {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInstanceIdW virtual profile={} value={value:?}",
                profile.label()
            ));
            return write_instance_id_w(
                &value,
                device_instance_id,
                device_instance_id_size,
                required_size,
            );
        }
    }
    ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_W
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_info_data,
                device_instance_id,
                device_instance_id_size,
                required_size,
            )
        })
}

pub unsafe extern "system" fn detoured_setupdi_get_device_instance_id_a(
    device_info_set: HANDLE,
    device_info_data: *mut c_void,
    device_instance_id: *mut u8,
    device_instance_id_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_info_data(device_info_data) {
        if let Some(value) =
            runtime_catalog().and_then(|catalog| catalog.device_instance_id(profile))
        {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInstanceIdA virtual profile={} value={value:?}",
                profile.label()
            ));
            return write_instance_id_a(
                &value,
                device_instance_id,
                device_instance_id_size,
                required_size,
            );
        }
    }
    ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_A
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_info_data,
                device_instance_id,
                device_instance_id_size,
                required_size,
            )
        })
}

pub unsafe extern "system" fn detoured_setupdi_get_device_property_w(
    device_info_set: HANDLE,
    device_info_data: *mut c_void,
    property_key: *const DEVPROPKEY,
    property_type: *mut u32,
    property_buffer: *mut u8,
    property_buffer_size: u32,
    required_size: *mut u32,
    flags: u32,
) -> BOOL {
    if let Some(profile) = virtual_profile_for_device_info_data(device_info_data) {
        if let Some(value) = device_property_value(profile, property_key) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDevicePropertyW virtual profile={} prop_type={}",
                profile.label(),
                value.prop_type
            ));
            return write_device_property_w(
                value,
                property_type,
                property_buffer,
                property_buffer_size,
                required_size,
            );
        }
    }
    ORIGINAL_SETUPDI_GET_DEVICE_PROPERTY_W
        .get()
        .copied()
        .map_or(0, |original| {
            original(
                device_info_set,
                device_info_data,
                property_key,
                property_type,
                property_buffer,
                property_buffer_size,
                required_size,
                flags,
            )
        })
}

fn runtime_catalog() -> Option<VirtualHidProfileCatalog> {
    state::catalog("SetupAPI catalog")
}

fn synthetic_profile_for_member(
    device_info_set: HANDLE,
    member_index: u32,
) -> Option<(VirtualHidProfile, VirtualHidProfileCatalog)> {
    let catalog = runtime_catalog()?;
    let base = synthetic_enum_base_index(device_info_set, member_index);
    let offset = member_index.checked_sub(base)? as usize;
    let profile = catalog.descriptors().get(offset)?.profile;
    Some((profile, catalog))
}

fn synthetic_enum_base_index(device_info_set: HANDLE, member_index: u32) -> u32 {
    if device_info_set == SYNTHETIC_DEVICE_INFO_SET {
        return 0;
    }

    let set_value = device_info_set as usize;
    let mut guard = match SYNTHETIC_ENUM_BASES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        Ok(guard) => guard,
        Err(_) => return member_index,
    };

    if let Some((_, base)) = guard
        .iter()
        .find(|(stored_set, _)| *stored_set == set_value)
    {
        return *base;
    }

    guard.push((set_value, member_index));
    member_index
}

unsafe fn synthesize_device_interface_detail_w(
    profile: VirtualHidProfile,
    device_interface_detail_data: *mut c_void,
    device_interface_detail_data_size: u32,
    required_size: *mut u32,
    device_info_data: *mut c_void,
) -> BOOL {
    let Some(path) = runtime_catalog().and_then(|catalog| catalog.device_path(profile)) else {
        SetLastError(ERROR_NO_MORE_ITEMS);
        return 0;
    };
    let required = 4 + ((path.encode_utf16().count() + 1) * 2) as u32;
    if !required_size.is_null() {
        *required_size = required;
    }
    write_synthetic_device_info_data(device_info_data, profile, "SyntheticDetailW");
    if device_interface_detail_data.is_null() || device_interface_detail_data_size < required {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    write_detail_w_path(device_interface_detail_data, &path);
    debug_line(&format!(
        "[crosspuck] SetupDiGetDeviceInterfaceDetailW synthetic profile={} path={path:?}",
        profile.label()
    ));
    1
}

unsafe fn synthesize_device_interface_detail_a(
    profile: VirtualHidProfile,
    device_interface_detail_data: *mut c_void,
    device_interface_detail_data_size: u32,
    required_size: *mut u32,
    device_info_data: *mut c_void,
) -> BOOL {
    let Some(path) = runtime_catalog().and_then(|catalog| catalog.device_path(profile)) else {
        SetLastError(ERROR_NO_MORE_ITEMS);
        return 0;
    };
    let required = 4 + path.len() as u32 + 1;
    if !required_size.is_null() {
        *required_size = required;
    }
    write_synthetic_device_info_data(device_info_data, profile, "SyntheticDetailA");
    if device_interface_detail_data.is_null() || device_interface_detail_data_size < required {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    write_detail_a_path(device_interface_detail_data, &path);
    debug_line(&format!(
        "[crosspuck] SetupDiGetDeviceInterfaceDetailA synthetic profile={} path={path:?}",
        profile.label()
    ));
    1
}

unsafe fn write_synthetic_device_interface_data(
    device_interface_data: *mut c_void,
    interface_class_guid: *const GUID,
    profile: VirtualHidProfile,
) -> bool {
    if device_interface_data.is_null() {
        return false;
    }
    let data = &mut *(device_interface_data as *mut SpDeviceInterfaceData);
    if data.cb_size == 0 {
        data.cb_size = mem::size_of::<SpDeviceInterfaceData>() as u32;
    }
    data.interface_class_guid = interface_class_guid
        .as_ref()
        .copied()
        .unwrap_or(HID_INTERFACE_GUID);
    data.flags = SPINT_ACTIVE;
    data.reserved = interface_reserved(profile);
    true
}

unsafe fn write_synthetic_device_info_data(
    device_info_data: *mut c_void,
    profile: VirtualHidProfile,
    source: &str,
) {
    if device_info_data.is_null() {
        return;
    }
    let data = &mut *(device_info_data as *mut SpDevinfoData);
    if data.cb_size == 0 {
        data.cb_size = mem::size_of::<SpDevinfoData>() as u32;
    }
    data.class_guid = HID_INTERFACE_GUID;
    data.dev_inst = synthetic_devinst(profile);
    data.reserved = 0;
    remember_virtual_devinst(data.dev_inst, profile, source);
}

unsafe fn write_detail_w_path(detail: *mut c_void, value: &str) {
    let path_ptr = (detail as *mut u8).add(4) as *mut u16;
    let mut written = 0;
    for unit in value.encode_utf16() {
        *path_ptr.add(written) = unit;
        written += 1;
    }
    *path_ptr.add(written) = 0;
}

unsafe fn write_detail_a_path(detail: *mut c_void, value: &str) {
    let path_ptr = (detail as *mut u8).add(4);
    let bytes = value.as_bytes();
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), path_ptr, bytes.len());
    *path_ptr.add(bytes.len()) = 0;
}

fn registry_property_value(profile: VirtualHidProfile, property: u32) -> Option<RegistryValue> {
    let catalog = runtime_catalog()?;
    let identity = catalog.identity();
    let value = match property {
        SPDRP_DEVICEDESC | SPDRP_FRIENDLYNAME => RegistryValue {
            reg_type: REG_SZ,
            entries: vec![identity.product.clone()],
        },
        SPDRP_MFG => RegistryValue {
            reg_type: REG_SZ,
            entries: vec![identity.manufacturer.clone()],
        },
        SPDRP_SERVICE => RegistryValue {
            reg_type: REG_SZ,
            entries: vec!["HidUsb".to_string()],
        },
        SPDRP_CLASS => RegistryValue {
            reg_type: REG_SZ,
            entries: vec!["HIDClass".to_string()],
        },
        SPDRP_ENUMERATOR_NAME => RegistryValue {
            reg_type: REG_SZ,
            entries: vec!["HID".to_string()],
        },
        SPDRP_HARDWAREID => RegistryValue {
            reg_type: REG_MULTI_SZ,
            entries: catalog.hardware_ids(profile)?,
        },
        SPDRP_COMPATIBLEIDS => RegistryValue {
            reg_type: REG_MULTI_SZ,
            entries: catalog.compatible_ids(profile)?,
        },
        SPDRP_LOCATION_PATHS => RegistryValue {
            reg_type: REG_MULTI_SZ,
            entries: vec![catalog.location_path(profile)?],
        },
        _ => return None,
    };
    Some(value)
}

fn device_property_value(
    profile: VirtualHidProfile,
    property_key: *const DEVPROPKEY,
) -> Option<DevicePropertyValue> {
    let key = unsafe { property_key.as_ref()? };
    let catalog = runtime_catalog()?;
    let identity = catalog.identity();
    let value = if devpropkey_eq(key, &DEVPKEY_DEVICE_DEVICE_DESC)
        || devpropkey_eq(key, &DEVPKEY_DEVICE_FRIENDLY_NAME)
        || devpropkey_eq(key, &DEVPKEY_DEVICE_BUS_REPORTED_DEVICE_DESC)
    {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING,
            entries: vec![identity.product.clone()],
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_MANUFACTURER) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING,
            entries: vec![identity.manufacturer.clone()],
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_INSTANCE_ID) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING,
            entries: vec![catalog.device_instance_id(profile)?],
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_SERVICE) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING,
            entries: vec!["HidUsb".to_string()],
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_CLASS) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING,
            entries: vec!["HIDClass".to_string()],
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_ENUMERATOR_NAME) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING,
            entries: vec!["HID".to_string()],
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_HARDWARE_IDS) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING_LIST,
            entries: catalog.hardware_ids(profile)?,
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_COMPATIBLE_IDS) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING_LIST,
            entries: catalog.compatible_ids(profile)?,
        }
    } else if devpropkey_eq(key, &DEVPKEY_DEVICE_LOCATION_PATHS) {
        DevicePropertyValue {
            prop_type: DEVPROP_TYPE_STRING_LIST,
            entries: vec![catalog.location_path(profile)?],
        }
    } else {
        return None;
    };
    Some(value)
}

unsafe fn write_registry_w(
    value: RegistryValue,
    reg_type: *mut u32,
    buffer: *mut u8,
    buffer_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if !reg_type.is_null() {
        *reg_type = value.reg_type;
    }
    let bytes = encode_w_registry(value.reg_type, &value.entries);
    write_byte_buffer(&bytes, buffer, buffer_size, required_size)
}

unsafe fn write_registry_a(
    value: RegistryValue,
    reg_type: *mut u32,
    buffer: *mut u8,
    buffer_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if !reg_type.is_null() {
        *reg_type = value.reg_type;
    }
    let bytes = encode_a_registry(value.reg_type, &value.entries);
    write_byte_buffer(&bytes, buffer, buffer_size, required_size)
}

unsafe fn write_device_property_w(
    value: DevicePropertyValue,
    property_type: *mut u32,
    buffer: *mut u8,
    buffer_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if !property_type.is_null() {
        *property_type = value.prop_type;
    }
    let bytes = if value.prop_type == DEVPROP_TYPE_STRING_LIST {
        encode_w_multi_sz(&value.entries)
    } else {
        encode_w_string(value.entries.first().map(String::as_str).unwrap_or(""))
    };
    write_byte_buffer(&bytes, buffer, buffer_size, required_size)
}

unsafe fn write_byte_buffer(
    bytes: &[u8],
    buffer: *mut u8,
    buffer_size: u32,
    required_size: *mut u32,
) -> BOOL {
    if !required_size.is_null() {
        *required_size = bytes.len() as u32;
    }
    if buffer.is_null() || buffer_size < bytes.len() as u32 {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
    1
}

unsafe fn write_instance_id_w(
    value: &str,
    buffer: *mut u16,
    buffer_chars: u32,
    required_chars: *mut u32,
) -> BOOL {
    let units = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if !required_chars.is_null() {
        *required_chars = units.len() as u32;
    }
    if buffer.is_null() || buffer_chars < units.len() as u32 {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(units.as_ptr(), buffer, units.len());
    1
}

unsafe fn write_instance_id_a(
    value: &str,
    buffer: *mut u8,
    buffer_chars: u32,
    required_chars: *mut u32,
) -> BOOL {
    let bytes = value.bytes().chain(std::iter::once(0)).collect::<Vec<_>>();
    if !required_chars.is_null() {
        *required_chars = bytes.len() as u32;
    }
    if buffer.is_null() || buffer_chars < bytes.len() as u32 {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
    1
}

fn encode_w_registry(reg_type: u32, entries: &[String]) -> Vec<u8> {
    if reg_type == REG_MULTI_SZ {
        encode_w_multi_sz(entries)
    } else {
        encode_w_string(entries.first().map(String::as_str).unwrap_or(""))
    }
}

fn encode_a_registry(reg_type: u32, entries: &[String]) -> Vec<u8> {
    if reg_type == REG_MULTI_SZ {
        encode_a_multi_sz(entries)
    } else {
        encode_a_string(entries.first().map(String::as_str).unwrap_or(""))
    }
}

fn encode_w_string(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for unit in value.encode_utf16().chain(std::iter::once(0)) {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn encode_w_multi_sz(entries: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        for unit in entry.encode_utf16().chain(std::iter::once(0)) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn encode_a_string(value: &str) -> Vec<u8> {
    let mut out = value.as_bytes().to_vec();
    out.push(0);
    out
}

fn encode_a_multi_sz(entries: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        out.extend_from_slice(entry.as_bytes());
        out.push(0);
    }
    out.push(0);
    out
}

unsafe fn virtual_profile_for_device_interface_data(
    device_interface_data: *mut c_void,
) -> Option<VirtualHidProfile> {
    if device_interface_data.is_null() {
        return None;
    }
    let data = &*(device_interface_data as *const SpDeviceInterfaceData);
    virtual_profile_for_interface_reserved(data.reserved)
}

unsafe fn virtual_profile_for_device_info_data(
    device_info_data: *mut c_void,
) -> Option<VirtualHidProfile> {
    let devinst = devinst_from_device_info_data(device_info_data)?;
    virtual_profile_for_devinst(devinst)
}

unsafe fn remember_virtual_device_info(device_info_data: *mut c_void, source: &str) {
    let Some(devinst) = devinst_from_device_info_data(device_info_data) else {
        return;
    };
    if let Some(profile) = virtual_profile_for_devinst(devinst) {
        remember_virtual_devinst(devinst, profile, source);
    }
}

unsafe fn devinst_from_device_info_data(device_info_data: *mut c_void) -> Option<u32> {
    if device_info_data.is_null() {
        return None;
    }
    let data = &*(device_info_data as *const SpDevinfoData);
    (data.cb_size >= 24).then_some(data.dev_inst)
}

fn virtual_profile_for_devinst(devinst: u32) -> Option<VirtualHidProfile> {
    VIRTUAL_DEVINSTS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()?
        .iter()
        .find_map(|(stored, profile)| (*stored == devinst).then_some(*profile))
}

fn remember_virtual_devinst(devinst: u32, profile: VirtualHidProfile, source: &str) {
    let Ok(mut guard) = VIRTUAL_DEVINSTS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    else {
        return;
    };
    if let Some((_, stored_profile)) = guard.iter_mut().find(|(stored, _)| *stored == devinst) {
        *stored_profile = profile;
    } else {
        guard.push((devinst, profile));
    }
    debug_line(&format!(
        "[crosspuck] SetupAPI remember devinst=0x{devinst:08X} profile={} source={source}",
        profile.label()
    ));
}

fn interface_reserved(profile: VirtualHidProfile) -> usize {
    let interface_number = match profile {
        VirtualHidProfile::Main => 2,
        VirtualHidProfile::Interface3 => 3,
        VirtualHidProfile::Interface4 => 4,
        VirtualHidProfile::Interface5 => 5,
        VirtualHidProfile::VendorDongle => 6,
    };
    VIRTUAL_INTERFACE_RESERVED_BASE | interface_number as usize
}

fn virtual_profile_for_interface_reserved(reserved: usize) -> Option<VirtualHidProfile> {
    if reserved & 0xFFFF_FFFF_FFFF_0000 != VIRTUAL_INTERFACE_RESERVED_BASE {
        return None;
    }
    match (reserved & 0xFFFF) as u8 {
        2 => Some(VirtualHidProfile::Main),
        3 => Some(VirtualHidProfile::Interface3),
        4 => Some(VirtualHidProfile::Interface4),
        5 => Some(VirtualHidProfile::Interface5),
        6 => Some(VirtualHidProfile::VendorDongle),
        _ => None,
    }
}

fn synthetic_devinst(profile: VirtualHidProfile) -> u32 {
    let interface_number = match profile {
        VirtualHidProfile::Main => 2,
        VirtualHidProfile::Interface3 => 3,
        VirtualHidProfile::Interface4 => 4,
        VirtualHidProfile::Interface5 => 5,
        VirtualHidProfile::VendorDongle => 6,
    };
    0x4350_0000 | interface_number
}

unsafe fn is_hid_interface_guid(guid: *const GUID) -> bool {
    guid.as_ref()
        .is_some_and(|guid| guid_eq(guid, &HID_INTERFACE_GUID))
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn devpropkey_eq(a: &DEVPROPKEY, b: &DEVPROPKEY) -> bool {
    guid_eq(&a.fmtid, &b.fmtid) && a.pid == b.pid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_double_null_terminated_wide_multi_sz() {
        let bytes = encode_w_multi_sz(&["A".to_string(), "B".to_string()]);
        assert_eq!(bytes, vec![0x41, 0, 0, 0, 0x42, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn encodes_double_null_terminated_ansi_multi_sz() {
        let bytes = encode_a_multi_sz(&["A".to_string(), "B".to_string()]);
        assert_eq!(bytes, b"A\0B\0\0");
    }
}
