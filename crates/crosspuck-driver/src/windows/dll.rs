use super::{hooks, log::debug_line, state};
use crosspuck_core::guest_driver::RuntimeConfig;
use std::ffi::c_void;
use std::panic;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    hinst: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    match reason {
        DLL_PROCESS_ATTACH => {
            DisableThreadLibraryCalls(hinst);
            let _ = panic::catch_unwind(attach);
        }
        DLL_PROCESS_DETACH => {
            if let Some(runtime) = state::runtime() {
                runtime.clear_bridge("dll detach");
            }
        }
        _ => {}
    }
    TRUE
}

fn attach() {
    let config = RuntimeConfig::from_env();
    let host_bridge_enabled = config.host_bridge_enabled;
    let host_bridge_required = config.host_bridge_required;
    let trace_reports = config.trace_reports;
    if !state::init_runtime(config) {
        debug_line("[crosspuck] driver runtime already initialized");
        return;
    }

    if let Err(error) = hooks::install() {
        debug_line(&format!("[crosspuck] hook install failed: {error}"));
    } else {
        debug_line("[crosspuck] hook install ok");
    }

    debug_line(&format!(
        "[crosspuck] crosspuck-driver attached host_bridge={} required={} trace={}",
        host_bridge_enabled, host_bridge_required, trace_reports
    ));
    if host_bridge_enabled {
        std::thread::spawn(|| {
            if let Some(runtime) = state::runtime() {
                match runtime.connect_bridge() {
                    Ok(()) => {
                        let snapshot = runtime.snapshot();
                        debug_line(&format!(
                            "[crosspuck] startup bridge connect ok identity={:?} profiles={} open_handles={}",
                            snapshot.identity_state,
                            snapshot.advertised_profiles,
                            snapshot.open_handles
                        ));
                    }
                    Err(error) => {
                        debug_line(&format!(
                            "[crosspuck] startup bridge connect failed: {error}"
                        ));
                    }
                }
            }
        });
    }
}
