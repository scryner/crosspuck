use super::state;
use crosspuck_core::guest_driver::VirtualHidProfile;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::HANDLE;

const VIRTUAL_MAIN_HANDLE_VALUE: isize = -0x4350_5543;
const VIRTUAL_IF3_HANDLE_VALUE: isize = -0x4350_5545;
const VIRTUAL_IF4_HANDLE_VALUE: isize = -0x4350_5546;
const VIRTUAL_IF5_HANDLE_VALUE: isize = -0x4350_5547;
const VIRTUAL_VENDOR_HANDLE_VALUE: isize = -0x4350_5544;

const FAKE_MAIN_PREPARSED_VALUE: usize = 0x4350_5543_4850_4D41;
const FAKE_IF3_PREPARSED_VALUE: usize = 0x4350_5543_4850_4933;
const FAKE_IF4_PREPARSED_VALUE: usize = 0x4350_5543_4850_4934;
const FAKE_IF5_PREPARSED_VALUE: usize = 0x4350_5543_4850_4935;
const FAKE_VENDOR_PREPARSED_VALUE: usize = 0x4350_5543_4850_564E;

#[allow(dead_code)]
pub fn handle_for_profile(profile: VirtualHidProfile) -> HANDLE {
    handle_value(profile) as usize as HANDLE
}

pub fn profile_for_handle(handle: HANDLE) -> Option<VirtualHidProfile> {
    let value = handle as isize;
    if value == VIRTUAL_MAIN_HANDLE_VALUE {
        Some(VirtualHidProfile::Main)
    } else if value == VIRTUAL_IF3_HANDLE_VALUE {
        Some(VirtualHidProfile::Interface3)
    } else if value == VIRTUAL_IF4_HANDLE_VALUE {
        Some(VirtualHidProfile::Interface4)
    } else if value == VIRTUAL_IF5_HANDLE_VALUE {
        Some(VirtualHidProfile::Interface5)
    } else if value == VIRTUAL_VENDOR_HANDLE_VALUE {
        Some(VirtualHidProfile::VendorDongle)
    } else {
        None
    }
}

pub fn profile_for_open_handle(handle: HANDLE) -> Option<VirtualHidProfile> {
    let profile = profile_for_handle(handle)?;
    state::runtime()
        .is_some_and(|runtime| runtime.is_profile_open(profile))
        .then_some(profile)
}

pub fn preparsed_for_profile(profile: VirtualHidProfile) -> *mut c_void {
    (match profile {
        VirtualHidProfile::Main => FAKE_MAIN_PREPARSED_VALUE,
        VirtualHidProfile::Interface3 => FAKE_IF3_PREPARSED_VALUE,
        VirtualHidProfile::Interface4 => FAKE_IF4_PREPARSED_VALUE,
        VirtualHidProfile::Interface5 => FAKE_IF5_PREPARSED_VALUE,
        VirtualHidProfile::VendorDongle => FAKE_VENDOR_PREPARSED_VALUE,
    }) as *mut c_void
}

pub fn profile_for_preparsed(preparsed_data: *mut c_void) -> Option<VirtualHidProfile> {
    let value = preparsed_data as usize;
    if value == FAKE_MAIN_PREPARSED_VALUE {
        Some(VirtualHidProfile::Main)
    } else if value == FAKE_IF3_PREPARSED_VALUE {
        Some(VirtualHidProfile::Interface3)
    } else if value == FAKE_IF4_PREPARSED_VALUE {
        Some(VirtualHidProfile::Interface4)
    } else if value == FAKE_IF5_PREPARSED_VALUE {
        Some(VirtualHidProfile::Interface5)
    } else if value == FAKE_VENDOR_PREPARSED_VALUE {
        Some(VirtualHidProfile::VendorDongle)
    } else {
        None
    }
}

fn handle_value(profile: VirtualHidProfile) -> isize {
    match profile {
        VirtualHidProfile::Main => VIRTUAL_MAIN_HANDLE_VALUE,
        VirtualHidProfile::Interface3 => VIRTUAL_IF3_HANDLE_VALUE,
        VirtualHidProfile::Interface4 => VIRTUAL_IF4_HANDLE_VALUE,
        VirtualHidProfile::Interface5 => VIRTUAL_IF5_HANDLE_VALUE,
        VirtualHidProfile::VendorDongle => VIRTUAL_VENDOR_HANDLE_VALUE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn maps_profiles_to_distinct_virtual_handles() {
        let mut handles = HashSet::new();

        for profile in VirtualHidProfile::ALL {
            let handle = handle_for_profile(profile);
            assert_eq!(profile_for_handle(handle), Some(profile));
            assert!(handles.insert(handle as isize));
        }
    }

    #[test]
    fn maps_profiles_to_distinct_fake_preparsed_pointers() {
        let mut pointers = HashSet::new();

        for profile in VirtualHidProfile::ALL {
            let preparsed = preparsed_for_profile(profile);
            assert_eq!(profile_for_preparsed(preparsed), Some(profile));
            assert!(pointers.insert(preparsed as usize));
        }
    }
}
