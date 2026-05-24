use crosspuck_core::guest_driver::GuestDriverError;
use windows_sys::Win32::Foundation::{SetLastError, ERROR_INVALID_HANDLE};

pub const ERROR_DEVICE_NOT_CONNECTED_CODE: u32 = 1167;

pub unsafe fn set_last_error_for(error: &GuestDriverError) {
    let code = match error {
        GuestDriverError::InvalidHandle => ERROR_INVALID_HANDLE,
        GuestDriverError::DeviceNotConnected
        | GuestDriverError::HostBridge(_)
        | GuestDriverError::ProfileUnavailable(_)
        | GuestDriverError::StatePoisoned(_) => ERROR_DEVICE_NOT_CONNECTED_CODE,
    };
    SetLastError(code);
}
