use super::hidd::HID_INTERFACE_GUID;
use super::log::{debug_line, info_line};
use super::state;
use crate::discovery_state::{DeviceChange, DiscoveryState};
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExA, GetWindowThreadProcessId, SendMessageTimeoutA, DBT_DEVICEARRIVAL,
    DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE, DEV_BROADCAST_DEVICEINTERFACE_A,
    HWND_MESSAGE, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, WM_DEVICECHANGE,
};

static STOP: AtomicBool = AtomicBool::new(false);

// A timed-out in-process SendMessageTimeout can outlive the sender's stack.
// This immutable payload stays valid for the lifetime of the pinned DLL.
static DEVICE_INTERFACE: DEV_BROADCAST_DEVICEINTERFACE_A = DEV_BROADCAST_DEVICEINTERFACE_A {
    dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_A>() as u32,
    dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE,
    dbcc_reserved: 0,
    dbcc_classguid: HID_INTERFACE_GUID,
    dbcc_name: [0],
};

pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

pub fn run() {
    let mut discovery = DiscoveryState::default();
    let mut waiting_for_receiver = false;
    info_line("[crosspuck] automatic discovery worker started");
    while !STOP.load(Ordering::Relaxed) {
        // Connect even if Steam stopped enumerating after its first failure.
        // The handshake includes host identity and the input-channel attach;
        // neither a TCP listener alone nor a missing Puck is advertised.
        let _ = state::catalog("automatic discovery");
        let live = state::runtime().and_then(|runtime| runtime.live_connection_generation());
        if let Some(change) = discovery.pending(live) {
            if notify_sdl(change) {
                discovery.acknowledge(change);
                waiting_for_receiver = false;
                info_line(&format!(
                    "[crosspuck] automatic discovery notified SDL change={change:?}"
                ));
            } else if !waiting_for_receiver {
                debug_line("[crosspuck] automatic discovery waiting for responsive SDL receiver");
                waiting_for_receiver = true;
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn notify_sdl(change: DeviceChange) -> bool {
    let event = match change {
        DeviceChange::Arrival(_) => DBT_DEVICEARRIVAL,
        DeviceChange::Removal => DBT_DEVICEREMOVECOMPLETE,
    };
    let mut window = ptr::null_mut();
    let mut found = false;
    let mut delivered = true;
    unsafe {
        loop {
            // SDL's receiver is a message-only window, omitted by EnumWindows
            // and HWND_BROADCAST. Never signal another process/bottle.
            window = FindWindowExA(
                HWND_MESSAGE,
                window,
                c"SDL_HIDAPI_DEVICE_DETECTION".as_ptr().cast(),
                ptr::null(),
            );
            if window.is_null() || STOP.load(Ordering::Relaxed) {
                break;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if pid != std::process::id() {
                continue;
            }
            found = true;
            let mut result = 0;
            delivered &= SendMessageTimeoutA(
                window,
                WM_DEVICECHANGE,
                event as usize,
                &DEVICE_INTERFACE as *const _ as isize,
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                500,
                &mut result,
            ) != 0;
        }
    }
    found && delivered
}
