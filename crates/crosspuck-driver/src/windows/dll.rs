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
            std::thread::spawn(|| {
                let _ = panic::catch_unwind(attach);
            });
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
    let config = RuntimeConfig::driver_from_env();
    let host_bridge_enabled = config.host_bridge_enabled;
    let host_bridge_required = config.host_bridge_required;
    let trace_reports = config.trace_reports;
    let connect_timeout_ms = config.connect_timeout.as_millis();
    let handshake_timeout_ms = config.handshake_timeout.as_millis();
    let io_timeout_ms = config.io_timeout.as_millis();
    let reconnect_interval_ms = config.lazy_reconnect_interval.as_millis();
    let process = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "<unknown>".to_string());
    let bridge_connect_allowed = process.eq_ignore_ascii_case("steam.exe");
    if !state::init_runtime(config, bridge_connect_allowed) {
        debug_line("[crosspuck] driver runtime already initialized");
        return;
    }

    if let Err(error) = hooks::install() {
        debug_line(&format!("[crosspuck] hook install failed: {error}"));
    } else {
        debug_line("[crosspuck] hook install ok");
    }

    debug_line(&format!(
        "[crosspuck] crosspuck-driver attached pid={} process={} host_bridge={} required={} trace={} bridge_connect_allowed={} connect_timeout_ms={} handshake_timeout_ms={} io_timeout_ms={} reconnect_interval_ms={}",
        std::process::id(),
        process,
        host_bridge_enabled,
        host_bridge_required,
        trace_reports,
        bridge_connect_allowed,
        connect_timeout_ms,
        handshake_timeout_ms,
        io_timeout_ms,
        reconnect_interval_ms
    ));
    if host_bridge_enabled {
        debug_line("[crosspuck] startup bridge connect skipped: lazy connect enabled");
        if bridge_connect_allowed {
            state::start_bridge_connector("steam startup");
        }
    }
}
