use super::buffers::{input_slice, output_slice, report_id_from_buffer};
use super::handles::{handle_for_profile, profile_for_handle};
use super::log::{debug_line, error_line, trace_line};
use super::proc::fn_from_mut;
use super::state;
use crosspuck_core::guest_driver::{
    path_may_be_virtual, VirtualHandleId, VirtualHidProfile, VirtualHidProfileCatalog,
};
use std::ffi::{c_char, c_int, c_void, CString};
use std::fmt;
use std::ptr;
use std::slice;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::core::PCSTR;
use windows_sys::Win32::System::LibraryLoader::LoadLibraryA;

type SdlHidCloseFn = unsafe extern "C" fn(*mut c_void);
type SdlHidEnumerateFn = unsafe extern "C" fn(u16, u16) -> *mut SdlHidDeviceInfo;
type SdlHidFreeEnumerationFn = unsafe extern "C" fn(*mut SdlHidDeviceInfo);
type SdlHidOpenPathFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type SdlHidSetNonblockingFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type SdlHidReadFn = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> c_int;
type SdlHidReadTimeoutFn = unsafe extern "C" fn(*mut c_void, *mut u8, usize, c_int) -> c_int;
type SdlHidWriteFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;
type SdlHidGetFeatureReportFn = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> c_int;
type SdlHidSendFeatureReportFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;

static ORIGINAL_SDL_HID_CLOSE: OnceLock<SdlHidCloseFn> = OnceLock::new();
static ORIGINAL_SDL_HID_ENUMERATE: OnceLock<SdlHidEnumerateFn> = OnceLock::new();
static ORIGINAL_SDL_HID_FREE_ENUMERATION: OnceLock<SdlHidFreeEnumerationFn> = OnceLock::new();
static ORIGINAL_SDL_HID_OPEN_PATH: OnceLock<SdlHidOpenPathFn> = OnceLock::new();
static ORIGINAL_SDL_HID_SET_NONBLOCKING: OnceLock<SdlHidSetNonblockingFn> = OnceLock::new();
static ORIGINAL_SDL_HID_READ: OnceLock<SdlHidReadFn> = OnceLock::new();
static ORIGINAL_SDL_HID_READ_TIMEOUT: OnceLock<SdlHidReadTimeoutFn> = OnceLock::new();
static ORIGINAL_SDL_HID_WRITE: OnceLock<SdlHidWriteFn> = OnceLock::new();
static ORIGINAL_SDL_HID_GET_FEATURE_REPORT: OnceLock<SdlHidGetFeatureReportFn> = OnceLock::new();
static ORIGINAL_SDL_HID_SEND_FEATURE_REPORT: OnceLock<SdlHidSendFeatureReportFn> = OnceLock::new();
static SDL_AUGMENTED_ENUMERATIONS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
static SDL_OPEN_SENTINELS: OnceLock<Mutex<Vec<(VirtualHidProfile, VirtualHandleId)>>> =
    OnceLock::new();
static SDL_FAILURE_LOGS: OnceLock<Mutex<Vec<SdlFailureLog>>> = OnceLock::new();
static SDL_SUCCESS_LOGS: OnceLock<Mutex<Vec<SdlSuccessLog>>> = OnceLock::new();
static SDL_READ_LOGS: OnceLock<Mutex<Vec<SdlReadLog>>> = OnceLock::new();

const SDL_FAILURE_LOG_INTERVAL: Duration = Duration::from_secs(2);
const SDL_SUCCESS_LOG_INTERVAL: Duration = Duration::from_secs(2);
const SDL_READ_LOG_INTERVAL: Duration = Duration::from_secs(2);
const SDL_VIRTUAL_BLOCKING_READ_MAX: Duration = Duration::from_millis(50);

struct SdlFailureLog {
    operation: &'static str,
    profile: VirtualHidProfile,
    error: String,
    last: Instant,
    suppressed: u32,
}

struct SdlSuccessLog {
    operation: &'static str,
    profile: VirtualHidProfile,
    detail: String,
    last: Instant,
    suppressed: u32,
}

struct SdlReadLog {
    profile: VirtualHidProfile,
    last: Instant,
    suppressed: u32,
}

#[repr(C)]
pub struct SdlHidDeviceInfo {
    path: *mut c_char,
    vendor_id: u16,
    product_id: u16,
    serial_number: *mut u16,
    release_number: u16,
    manufacturer_string: *mut u16,
    product_string: *mut u16,
    usage_page: u16,
    usage: u16,
    interface_number: c_int,
    interface_class: c_int,
    interface_subclass: c_int,
    interface_protocol: c_int,
    bus_type: c_int,
    next: *mut SdlHidDeviceInfo,
}

pub fn load_sdl3() {
    let Ok(name) = CString::new("SDL3.dll") else {
        return;
    };
    let module = unsafe { LoadLibraryA(name.as_ptr() as PCSTR) };
    debug_line(&format!(
        "[crosspuck] SDL3.dll load for hid hooks -> 0x{:016X}",
        module as usize
    ));
}

pub fn log_optional_hook_installed(module: &str, proc_name: &str) {
    debug_line(&format!(
        "[crosspuck] optional hook installed {module}!{proc_name}"
    ));
}

pub fn log_optional_hook_error(module: &str, proc_name: &str, error: &str) {
    debug_line(&format!(
        "[crosspuck] optional hook {module}!{proc_name} failed: {error}"
    ));
}

pub fn set_original_sdl_hid_close(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_CLOSE
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_close trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_enumerate(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_ENUMERATE
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_enumerate trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_free_enumeration(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_FREE_ENUMERATION
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_free_enumeration trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_open_path(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_OPEN_PATH
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_open_path trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_set_nonblocking(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_SET_NONBLOCKING
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_set_nonblocking trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_read(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_READ
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_read trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_read_timeout(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_READ_TIMEOUT
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_read_timeout trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_write(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_WRITE
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_write trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_get_feature_report(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_GET_FEATURE_REPORT
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_get_feature_report trampoline already initialized".to_string())
}

pub fn set_original_sdl_hid_send_feature_report(ptr: *mut c_void) -> Result<(), String> {
    ORIGINAL_SDL_HID_SEND_FEATURE_REPORT
        .set(unsafe { fn_from_mut(ptr) })
        .map_err(|_| "SDL_hid_send_feature_report trampoline already initialized".to_string())
}

pub unsafe extern "C" fn detoured_sdl_hid_close(device: *mut c_void) {
    let profile = profile_for_sdl_device(device);
    if let Some(profile) = profile {
        close_sdl_profile(profile);
        debug_line(&format!(
            "[crosspuck] SDL_hid_close virtual profile={} device={device:p}",
            profile.label()
        ));
        return;
    }

    if let Some(original) = ORIGINAL_SDL_HID_CLOSE.get().copied() {
        original(device);
    }
}

pub unsafe extern "C" fn detoured_sdl_hid_enumerate(
    vendor_id: u16,
    product_id: u16,
) -> *mut SdlHidDeviceInfo {
    let original = ORIGINAL_SDL_HID_ENUMERATE
        .get()
        .copied()
        .map_or(ptr::null_mut(), |original| original(vendor_id, product_id));
    let Some(catalog) = state::catalog("SDL_hid_enumerate") else {
        return original;
    };
    if !sdl_query_matches_catalog(&catalog, vendor_id, product_id) {
        return original;
    }

    let augmented = augment_sdl_hid_enumeration(original, &catalog);
    debug_line(&format!(
        "[crosspuck] SDL_hid_enumerate vid=0x{vendor_id:04X} pid=0x{product_id:04X} original={original:p} returned={augmented:p}"
    ));
    augmented
}

pub unsafe extern "C" fn detoured_sdl_hid_free_enumeration(device_info: *mut SdlHidDeviceInfo) {
    if is_augmented_sdl_enumeration(device_info) {
        debug_line(&format!(
            "[crosspuck] SDL_hid_free_enumeration augmented head={device_info:p} leaked"
        ));
        return;
    }

    if let Some(original) = ORIGINAL_SDL_HID_FREE_ENUMERATION.get().copied() {
        original(device_info);
    }
}

pub unsafe extern "C" fn detoured_sdl_hid_open_path(path: *const c_char) -> *mut c_void {
    let path_text = narrow_z_to_string(path as PCSTR);
    if let Some(path) = path_text.as_deref() {
        if let Some(device) = open_synthetic_path(path) {
            return device;
        }
    }

    ORIGINAL_SDL_HID_OPEN_PATH
        .get()
        .copied()
        .map_or(ptr::null_mut(), |original| original(path))
}

pub unsafe extern "C" fn detoured_sdl_hid_set_nonblocking(
    device: *mut c_void,
    nonblock: c_int,
) -> c_int {
    let Some(profile) = profile_for_sdl_device(device) else {
        return ORIGINAL_SDL_HID_SET_NONBLOCKING
            .get()
            .copied()
            .map_or(-1, |original| original(device, nonblock));
    };

    log_sdl_success(
        profile,
        "SDL_hid_set_nonblocking",
        &format_args!("nonblock={nonblock}"),
    );
    0
}

pub unsafe extern "C" fn detoured_sdl_hid_read(
    device: *mut c_void,
    data: *mut u8,
    len: usize,
) -> c_int {
    let Some(profile) = profile_for_sdl_device(device) else {
        return ORIGINAL_SDL_HID_READ
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len));
    };

    detoured_sdl_hid_read_common(profile, data, len, "SDL_hid_read", None)
}

pub unsafe extern "C" fn detoured_sdl_hid_read_timeout(
    device: *mut c_void,
    data: *mut u8,
    len: usize,
    milliseconds: c_int,
) -> c_int {
    let Some(profile) = profile_for_sdl_device(device) else {
        return ORIGINAL_SDL_HID_READ_TIMEOUT
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len, milliseconds));
    };

    detoured_sdl_hid_read_common(
        profile,
        data,
        len,
        "SDL_hid_read_timeout",
        Some(milliseconds),
    )
}

unsafe fn detoured_sdl_hid_read_common(
    profile: VirtualHidProfile,
    data: *mut u8,
    len: usize,
    operation: &'static str,
    milliseconds: Option<c_int>,
) -> c_int {
    let Some(buffer_len) = len_to_u32(len) else {
        return -1;
    };
    let Some(output) = output_slice(data as *mut c_void, buffer_len) else {
        return -1;
    };

    let timeout = milliseconds.unwrap_or(-1);
    let deadline = match milliseconds {
        Some(0) => Some(Instant::now()),
        Some(value) if value > 0 => Some(Instant::now() + Duration::from_millis(value as u64)),
        _ => Some(Instant::now() + SDL_VIRTUAL_BLOCKING_READ_MAX),
    };
    loop {
        match read_sdl_report(profile, output, operation) {
            Ok(Some(count)) => {
                log_sdl_read_success(profile, operation, timeout, len, count);
                return count_to_c_int(count);
            }
            Ok(None) if timeout == 0 => {
                return 0;
            }
            Ok(None) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                return 0;
            }
            Ok(None) => thread::sleep(Duration::from_millis(1)),
            Err(()) => return -1,
        }
    }
}

pub unsafe extern "C" fn detoured_sdl_hid_write(
    device: *mut c_void,
    data: *const u8,
    len: usize,
) -> c_int {
    let Some(profile) = profile_for_sdl_device(device) else {
        return ORIGINAL_SDL_HID_WRITE
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len));
    };
    let Some(buffer_len) = len_to_u32(len) else {
        return -1;
    };
    let Some(payload) = input_slice(data as *const c_void, buffer_len) else {
        return -1;
    };
    let Some(runtime) = state::runtime() else {
        return -1;
    };

    match runtime.write_report(profile, payload) {
        Ok(count) => {
            trace_virtual_payload(profile, "SDL_hid_write", payload);
            log_sdl_success(
                profile,
                "SDL_hid_write",
                &format_args!("requested={} accepted={count}", payload.len()),
            );
            count_to_c_int(usize::from(count))
        }
        Err(error) => {
            log_sdl_failure(profile, "SDL_hid_write", &error);
            -1
        }
    }
}

pub unsafe extern "C" fn detoured_sdl_hid_get_feature_report(
    device: *mut c_void,
    data: *mut u8,
    len: usize,
) -> c_int {
    let Some(profile) = profile_for_sdl_device(device) else {
        return ORIGINAL_SDL_HID_GET_FEATURE_REPORT
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len));
    };
    let Some(buffer_len) = len_to_u32(len) else {
        return -1;
    };
    let report_id = report_id_from_buffer(data as *const c_void, buffer_len);
    let Some(output) = output_slice(data as *mut c_void, buffer_len) else {
        return -1;
    };
    let Some(runtime) = state::runtime() else {
        return -1;
    };

    match runtime.copy_feature_report(profile, report_id, output) {
        Ok(count) => {
            trace_virtual_payload(profile, "SDL_hid_get_feature_report", &output[..count]);
            log_sdl_success(
                profile,
                "SDL_hid_get_feature_report",
                &format_args!(
                    "report_id=0x{report_id:02X} requested={} returned={count}",
                    len
                ),
            );
            count_to_c_int(count)
        }
        Err(error) => {
            log_sdl_failure(
                profile,
                "SDL_hid_get_feature_report",
                &format_args!("report_id=0x{report_id:02X} {error}"),
            );
            -1
        }
    }
}

pub unsafe extern "C" fn detoured_sdl_hid_send_feature_report(
    device: *mut c_void,
    data: *const u8,
    len: usize,
) -> c_int {
    let Some(profile) = profile_for_sdl_device(device) else {
        return ORIGINAL_SDL_HID_SEND_FEATURE_REPORT
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len));
    };
    let Some(buffer_len) = len_to_u32(len) else {
        return -1;
    };
    let Some(payload) = input_slice(data as *const c_void, buffer_len) else {
        return -1;
    };
    let Some(runtime) = state::runtime() else {
        return -1;
    };

    match runtime.set_feature(profile, payload) {
        Ok(count) => {
            trace_virtual_payload(profile, "SDL_hid_send_feature_report", payload);
            log_sdl_success(
                profile,
                "SDL_hid_send_feature_report",
                &format_args!("requested={} accepted={count}", payload.len()),
            );
            count_to_c_int(usize::from(count))
        }
        Err(error) => {
            log_sdl_failure(profile, "SDL_hid_send_feature_report", &error);
            -1
        }
    }
}

unsafe fn augment_sdl_hid_enumeration(
    head: *mut SdlHidDeviceInfo,
    catalog: &VirtualHidProfileCatalog,
) -> *mut SdlHidDeviceInfo {
    let mut seen = Vec::new();
    let mut tail = ptr::null_mut();
    let mut cursor = head;
    while !cursor.is_null() {
        tail = cursor;
        let path = narrow_z_to_string((*cursor).path as PCSTR).unwrap_or_default();
        if let Some(profile) = catalog.profile_for_path(&path) {
            rewrite_sdl_hid_device_info(cursor, catalog, profile);
            if !seen.contains(&profile) {
                seen.push(profile);
            }
        }
        cursor = (*cursor).next;
    }

    let missing_profiles = catalog
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.profile)
        .filter(|profile| !seen.contains(profile))
        .collect::<Vec<_>>();
    let synthetic_head = build_sdl_hid_info_list(catalog, &missing_profiles);
    let result = if head.is_null() {
        synthetic_head
    } else {
        if !tail.is_null() {
            (*tail).next = synthetic_head;
        }
        head
    };

    remember_augmented_sdl_enumeration(result);
    result
}

unsafe fn rewrite_sdl_hid_device_info(
    device_info: *mut SdlHidDeviceInfo,
    catalog: &VirtualHidProfileCatalog,
    profile: VirtualHidProfile,
) {
    if device_info.is_null() {
        return;
    }
    let Some(descriptor) = catalog.descriptor(profile) else {
        return;
    };
    let Some(path) = catalog.device_path(profile) else {
        return;
    };
    let identity = catalog.identity();

    (*device_info).path = leak_c_string(&path);
    (*device_info).vendor_id = identity.vendor_id;
    (*device_info).product_id = identity.product_id;
    (*device_info).serial_number = leak_wide_string(&identity.serial);
    (*device_info).release_number = identity.version_number;
    (*device_info).manufacturer_string = leak_wide_string(&identity.manufacturer);
    (*device_info).product_string = leak_wide_string(&identity.product);
    (*device_info).usage_page = descriptor.usage_page;
    (*device_info).usage = descriptor.usage;
    (*device_info).interface_number = c_int::from(descriptor.interface_number);
    (*device_info).interface_class = 0;
    (*device_info).interface_subclass = 0;
    (*device_info).interface_protocol = 0;
    (*device_info).bus_type = 1;
    debug_line(&format!(
        "[crosspuck] SDL_hid_enumerate rewrite profile={} path={path:?}",
        profile.label()
    ));
}

unsafe fn build_sdl_hid_info_list(
    catalog: &VirtualHidProfileCatalog,
    profiles: &[VirtualHidProfile],
) -> *mut SdlHidDeviceInfo {
    let mut head: *mut SdlHidDeviceInfo = ptr::null_mut();
    let mut tail: *mut SdlHidDeviceInfo = ptr::null_mut();
    let identity = catalog.identity();
    for profile in profiles.iter().copied() {
        let Some(descriptor) = catalog.descriptor(profile) else {
            continue;
        };
        let Some(path) = catalog.device_path(profile) else {
            continue;
        };
        let node = Box::into_raw(Box::new(SdlHidDeviceInfo {
            path: leak_c_string(&path),
            vendor_id: identity.vendor_id,
            product_id: identity.product_id,
            serial_number: leak_wide_string(&identity.serial),
            release_number: identity.version_number,
            manufacturer_string: leak_wide_string(&identity.manufacturer),
            product_string: leak_wide_string(&identity.product),
            usage_page: descriptor.usage_page,
            usage: descriptor.usage,
            interface_number: c_int::from(descriptor.interface_number),
            interface_class: 0,
            interface_subclass: 0,
            interface_protocol: 0,
            bus_type: 1,
            next: ptr::null_mut(),
        }));

        if head.is_null() {
            head = node;
        } else {
            (*tail).next = node;
        }
        tail = node;
        debug_line(&format!(
            "[crosspuck] SDL_hid_enumerate append synthetic profile={} path={path:?}",
            profile.label()
        ));
    }
    head
}

unsafe fn open_synthetic_path(path: &str) -> Option<*mut c_void> {
    if !path_may_be_virtual(path) {
        return None;
    }
    let runtime = state::runtime()?;
    let catalog = state::catalog_if_connected("SDL_hid_open_path")?;
    let profile = catalog.profile_for_path(path)?;
    match runtime.open_profile(profile) {
        Ok(handle_id) => {
            remember_sdl_profile(profile, handle_id);
            let device = handle_for_profile(profile);
            debug_line(&format!(
                "[crosspuck] SDL_hid_open_path virtual profile={} device={device:p} path={path:?}",
                profile.label()
            ));
            Some(device)
        }
        Err(error) => {
            error_line(&format!(
                "[crosspuck] SDL_hid_open_path failed path={path:?} profile={} error={error}",
                profile.label()
            ));
            None
        }
    }
}

fn remember_sdl_profile(profile: VirtualHidProfile, handle_id: VirtualHandleId) {
    if let Ok(mut handles) = SDL_OPEN_SENTINELS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        handles.push((profile, handle_id));
    }
}

fn close_sdl_profile(profile: VirtualHidProfile) -> bool {
    let Some(runtime) = state::runtime() else {
        return false;
    };
    let Some(handles) = SDL_OPEN_SENTINELS.get() else {
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
            "[crosspuck] SDL_hid_close core handle profile={} closed={closed}",
            profile.label()
        ));
        return closed;
    }
    debug_line(&format!(
        "[crosspuck] SDL_hid_close core handle profile={} closed=false",
        profile.label()
    ));
    false
}

fn profile_for_sdl_device(device: *mut c_void) -> Option<VirtualHidProfile> {
    let profile = profile_for_handle(device as _)?;
    let handles = SDL_OPEN_SENTINELS.get()?;
    handles
        .lock()
        .is_ok_and(|handles| handles.iter().any(|(stored, _)| *stored == profile))
        .then_some(profile)
}

fn sdl_query_matches_catalog(
    catalog: &VirtualHidProfileCatalog,
    vendor_id: u16,
    product_id: u16,
) -> bool {
    let identity = catalog.identity();
    (vendor_id == 0 || vendor_id == identity.vendor_id || vendor_id == 0x845e)
        && (product_id == 0
            || product_id == identity.product_id
            || product_id == 0x0001
            || product_id == 0x0002)
}

fn read_sdl_report(
    profile: VirtualHidProfile,
    output: &mut [u8],
    operation: &'static str,
) -> Result<Option<usize>, ()> {
    let Some(runtime) = state::runtime() else {
        return Err(());
    };
    match runtime.copy_next_input_report(profile, output) {
        Ok(result) => Ok(result),
        Err(error) => {
            log_sdl_failure(profile, operation, &error);
            Err(())
        }
    }
}

fn remember_augmented_sdl_enumeration(head: *mut SdlHidDeviceInfo) {
    if head.is_null() {
        return;
    }
    if let Ok(mut heads) = SDL_AUGMENTED_ENUMERATIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        let value = head as usize;
        if !heads.contains(&value) {
            heads.push(value);
        }
    }
}

fn is_augmented_sdl_enumeration(head: *mut SdlHidDeviceInfo) -> bool {
    let Some(heads) = SDL_AUGMENTED_ENUMERATIONS.get() else {
        return false;
    };
    let value = head as usize;
    heads.lock().is_ok_and(|heads| heads.contains(&value))
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

fn leak_c_string(value: &str) -> *mut c_char {
    CString::new(value)
        .unwrap_or_else(|_| CString::new("").expect("empty CString is valid"))
        .into_raw()
}

fn leak_wide_string(value: &str) -> *mut u16 {
    let mut units = value.encode_utf16().collect::<Vec<_>>();
    units.push(0);
    Box::leak(units.into_boxed_slice()).as_mut_ptr()
}

fn len_to_u32(len: usize) -> Option<u32> {
    u32::try_from(len).ok()
}

fn count_to_c_int(count: usize) -> c_int {
    count.min(c_int::MAX as usize) as c_int
}

fn log_sdl_failure(profile: VirtualHidProfile, operation: &'static str, error: &dyn fmt::Display) {
    let now = Instant::now();
    let error = error.to_string();
    let logs = SDL_FAILURE_LOGS.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut logs) = logs.lock() else {
        error_line(&format!(
            "[crosspuck] {operation} failed profile={} error={error}",
            profile.label()
        ));
        return;
    };

    let Some(entry) = logs.iter_mut().find(|entry| {
        entry.operation == operation && entry.profile == profile && entry.error == error
    }) else {
        logs.push(SdlFailureLog {
            operation,
            profile,
            error: error.clone(),
            last: now,
            suppressed: 0,
        });
        error_line(&format!(
            "[crosspuck] {operation} failed profile={} error={error}",
            profile.label()
        ));
        return;
    };

    if now.duration_since(entry.last) < SDL_FAILURE_LOG_INTERVAL {
        entry.suppressed = entry.suppressed.saturating_add(1);
        return;
    }

    let suppressed = entry.suppressed;
    entry.last = now;
    entry.suppressed = 0;
    if suppressed == 0 {
        error_line(&format!(
            "[crosspuck] {operation} failed profile={} error={error}",
            profile.label()
        ));
    } else {
        error_line(&format!(
            "[crosspuck] {operation} failed profile={} error={error} suppressed={suppressed}",
            profile.label()
        ));
    }
}

fn log_sdl_success(profile: VirtualHidProfile, operation: &'static str, detail: &dyn fmt::Display) {
    let now = Instant::now();
    let detail = detail.to_string();
    let logs = SDL_SUCCESS_LOGS.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut logs) = logs.lock() else {
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} {detail}",
            profile.label()
        ));
        return;
    };

    let Some(entry) = logs.iter_mut().find(|entry| {
        entry.operation == operation && entry.profile == profile && entry.detail == detail
    }) else {
        logs.push(SdlSuccessLog {
            operation,
            profile,
            detail: detail.clone(),
            last: now,
            suppressed: 0,
        });
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} {detail}",
            profile.label()
        ));
        return;
    };

    if now.duration_since(entry.last) < SDL_SUCCESS_LOG_INTERVAL {
        entry.suppressed = entry.suppressed.saturating_add(1);
        return;
    }

    let suppressed = entry.suppressed;
    entry.last = now;
    entry.suppressed = 0;
    if suppressed == 0 {
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} {detail}",
            profile.label()
        ));
    } else {
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} {detail} suppressed={suppressed}",
            profile.label()
        ));
    }
}

fn log_sdl_read_success(
    profile: VirtualHidProfile,
    operation: &'static str,
    milliseconds: c_int,
    requested: usize,
    returned: usize,
) {
    let now = Instant::now();
    let logs = SDL_READ_LOGS.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut logs) = logs.lock() else {
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} timeout={} requested={} returned={returned}",
            profile.label(),
            milliseconds,
            requested
        ));
        return;
    };

    let Some(entry) = logs.iter_mut().find(|entry| entry.profile == profile) else {
        logs.push(SdlReadLog {
            profile,
            last: now,
            suppressed: 0,
        });
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} timeout={} requested={} returned={returned}",
            profile.label(),
            milliseconds,
            requested
        ));
        return;
    };

    if now.duration_since(entry.last) < SDL_READ_LOG_INTERVAL {
        entry.suppressed = entry.suppressed.saturating_add(1);
        return;
    }

    let suppressed = entry.suppressed;
    entry.last = now;
    entry.suppressed = 0;
    if suppressed == 0 {
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} timeout={} requested={} returned={returned}",
            profile.label(),
            milliseconds,
            requested
        ));
    } else {
        debug_line(&format!(
            "[crosspuck] {operation} virtual profile={} timeout={} requested={} returned={returned} suppressed={suppressed}",
            profile.label(),
            milliseconds,
            requested
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
