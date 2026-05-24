use super::{kernel32, sdl, setupapi};
use min_hook_rs::{create_hook_api, enable_hook, initialize};
use std::ffi::c_void;

pub fn install() -> Result<(), String> {
    initialize().map_err(|error| format!("minhook initialize failed: {error:?}"))?;
    install_hook(
        "kernel32.dll",
        "CreateFileW",
        kernel32::detoured_create_file_w as *mut c_void,
        kernel32::set_original_create_file_w,
    )?;
    install_hook(
        "kernel32.dll",
        "CreateFileA",
        kernel32::detoured_create_file_a as *mut c_void,
        kernel32::set_original_create_file_a,
    )?;
    install_hook(
        "kernel32.dll",
        "ReadFile",
        kernel32::detoured_read_file as *mut c_void,
        kernel32::set_original_read_file,
    )?;
    install_hook(
        "kernel32.dll",
        "WriteFile",
        kernel32::detoured_write_file as *mut c_void,
        kernel32::set_original_write_file,
    )?;
    install_hook(
        "kernel32.dll",
        "CloseHandle",
        kernel32::detoured_close_handle as *mut c_void,
        kernel32::set_original_close_handle,
    )?;
    install_hook(
        "kernel32.dll",
        "DeviceIoControl",
        kernel32::detoured_device_io_control as *mut c_void,
        kernel32::set_original_device_io_control,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetClassDevsW",
        setupapi::detoured_setupdi_get_class_devs_w as *mut c_void,
        setupapi::set_original_setupdi_get_class_devs_w,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetClassDevsA",
        setupapi::detoured_setupdi_get_class_devs_a as *mut c_void,
        setupapi::set_original_setupdi_get_class_devs_a,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiEnumDeviceInterfaces",
        setupapi::detoured_setupdi_enum_device_interfaces as *mut c_void,
        setupapi::set_original_setupdi_enum_device_interfaces,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDeviceInterfaceDetailW",
        setupapi::detoured_setupdi_get_device_interface_detail_w as *mut c_void,
        setupapi::set_original_setupdi_get_device_interface_detail_w,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDeviceInterfaceDetailA",
        setupapi::detoured_setupdi_get_device_interface_detail_a as *mut c_void,
        setupapi::set_original_setupdi_get_device_interface_detail_a,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiEnumDeviceInfo",
        setupapi::detoured_setupdi_enum_device_info as *mut c_void,
        setupapi::set_original_setupdi_enum_device_info,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDeviceRegistryPropertyW",
        setupapi::detoured_setupdi_get_device_registry_property_w as *mut c_void,
        setupapi::set_original_setupdi_get_device_registry_property_w,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDeviceRegistryPropertyA",
        setupapi::detoured_setupdi_get_device_registry_property_a as *mut c_void,
        setupapi::set_original_setupdi_get_device_registry_property_a,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDeviceInstanceIdW",
        setupapi::detoured_setupdi_get_device_instance_id_w as *mut c_void,
        setupapi::set_original_setupdi_get_device_instance_id_w,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDeviceInstanceIdA",
        setupapi::detoured_setupdi_get_device_instance_id_a as *mut c_void,
        setupapi::set_original_setupdi_get_device_instance_id_a,
    )?;
    install_hook(
        "setupapi.dll",
        "SetupDiGetDevicePropertyW",
        setupapi::detoured_setupdi_get_device_property_w as *mut c_void,
        setupapi::set_original_setupdi_get_device_property_w,
    )?;
    install_sdl_hooks();
    Ok(())
}

fn install_hook(
    module: &str,
    proc_name: &str,
    detour: *mut c_void,
    set_original: impl FnOnce(*mut c_void) -> Result<(), String>,
) -> Result<(), String> {
    let (trampoline, target) = create_hook_api(module, proc_name, detour)
        .map_err(|error| format!("create hook {module}!{proc_name} failed: {error:?}"))?;
    set_original(trampoline)?;
    enable_hook(target)
        .map_err(|error| format!("enable hook {module}!{proc_name} failed: {error:?}"))?;
    Ok(())
}

fn install_optional_hook(
    module: &str,
    proc_name: &str,
    detour: *mut c_void,
    set_original: impl FnOnce(*mut c_void) -> Result<(), String>,
) {
    let Ok((trampoline, target)) = create_hook_api(module, proc_name, detour) else {
        return;
    };
    if let Err(error) = set_original(trampoline) {
        sdl::log_optional_hook_error(module, proc_name, &error);
        return;
    }
    if let Err(error) = enable_hook(target) {
        sdl::log_optional_hook_error(module, proc_name, &format!("{error:?}"));
        return;
    }
    sdl::log_optional_hook_installed(module, proc_name);
}

fn install_sdl_hooks() {
    sdl::load_sdl3();
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_close",
        sdl::detoured_sdl_hid_close as *mut c_void,
        sdl::set_original_sdl_hid_close,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_enumerate",
        sdl::detoured_sdl_hid_enumerate as *mut c_void,
        sdl::set_original_sdl_hid_enumerate,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_free_enumeration",
        sdl::detoured_sdl_hid_free_enumeration as *mut c_void,
        sdl::set_original_sdl_hid_free_enumeration,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_open_path",
        sdl::detoured_sdl_hid_open_path as *mut c_void,
        sdl::set_original_sdl_hid_open_path,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_read_timeout",
        sdl::detoured_sdl_hid_read_timeout as *mut c_void,
        sdl::set_original_sdl_hid_read_timeout,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_write",
        sdl::detoured_sdl_hid_write as *mut c_void,
        sdl::set_original_sdl_hid_write,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_get_feature_report",
        sdl::detoured_sdl_hid_get_feature_report as *mut c_void,
        sdl::set_original_sdl_hid_get_feature_report,
    );
    install_optional_hook(
        "SDL3.dll",
        "SDL_hid_send_feature_report",
        sdl::detoured_sdl_hid_send_feature_report as *mut c_void,
        sdl::set_original_sdl_hid_send_feature_report,
    );
}
