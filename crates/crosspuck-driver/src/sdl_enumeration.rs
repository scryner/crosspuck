//! Rust-owned copies of SDL HID enumerations. Never mix SDL's allocator with
//! Rust allocations or rewrite strings owned by SDL in place.
use crosspuck_core::guest_driver::{VirtualHidProfile, VirtualHidProfileCatalog};
use std::ffi::{c_char, c_int, CStr, CString};
use std::{ptr, slice};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SdlHidDeviceInfo {
    pub path: *mut c_char,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: *mut u16,
    pub release_number: u16,
    pub manufacturer_string: *mut u16,
    pub product_string: *mut u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface_number: c_int,
    pub interface_class: c_int,
    pub interface_subclass: c_int,
    pub interface_protocol: c_int,
    pub bus_type: c_int,
    pub next: *mut SdlHidDeviceInfo,
}

struct OwnedNode {
    info: Box<SdlHidDeviceInfo>,
    _path: Option<CString>,
    _serial: Option<Box<[u16]>>,
    _manufacturer: Option<Box<[u16]>>,
    _product: Option<Box<[u16]>>,
}

impl OwnedNode {
    fn new(
        mut info: SdlHidDeviceInfo,
        path: Option<CString>,
        mut serial: Option<Box<[u16]>>,
        mut manufacturer: Option<Box<[u16]>>,
        mut product: Option<Box<[u16]>>,
    ) -> Self {
        info.path = path
            .as_ref()
            .map_or(ptr::null_mut(), |s| s.as_ptr().cast_mut());
        info.serial_number = serial.as_mut().map_or(ptr::null_mut(), |s| s.as_mut_ptr());
        info.manufacturer_string = manufacturer
            .as_mut()
            .map_or(ptr::null_mut(), |s| s.as_mut_ptr());
        info.product_string = product.as_mut().map_or(ptr::null_mut(), |s| s.as_mut_ptr());
        info.next = ptr::null_mut();
        Self {
            info: Box::new(info),
            _path: path,
            _serial: serial,
            _manufacturer: manufacturer,
            _product: product,
        }
    }

    unsafe fn copy_native(info: &SdlHidDeviceInfo) -> Self {
        Self::new(
            *info,
            (!info.path.is_null()).then(|| CStr::from_ptr(info.path).to_owned()),
            copy_wide(info.serial_number),
            copy_wide(info.manufacturer_string),
            copy_wide(info.product_string),
        )
    }

    fn virtual_device(
        catalog: &VirtualHidProfileCatalog,
        profile: VirtualHidProfile,
    ) -> Option<Self> {
        let descriptor = catalog.descriptor(profile)?;
        let identity = catalog.identity();
        Some(Self::new(
            SdlHidDeviceInfo {
                vendor_id: identity.vendor_id,
                product_id: identity.product_id,
                release_number: identity.version_number,
                usage_page: descriptor.usage_page,
                usage: descriptor.usage,
                interface_number: c_int::from(descriptor.interface_number),
                bus_type: 1,
                ..SdlHidDeviceInfo::default()
            },
            Some(CString::new(catalog.device_path(profile)?).ok()?),
            Some(wide(&identity.serial)),
            Some(wide(&identity.manufacturer)),
            Some(wide(&identity.product)),
        ))
    }
}

pub(crate) struct OwnedSdlEnumeration {
    nodes: Vec<OwnedNode>,
}

// Every pointer in the published list refers to an allocation owned by nodes.
// Moving the owner preserves those allocations. The registry transfers unique
// ownership under its mutex; SDL callers must finish reading before freeing.
unsafe impl Send for OwnedSdlEnumeration {}

impl OwnedSdlEnumeration {
    /// `original` must be a valid SDL enumeration for the duration of this call.
    /// The returned list owns all its nodes/strings; the original is untouched
    /// and may immediately be released by SDL's original free function.
    pub(crate) unsafe fn augment(
        mut original: *mut SdlHidDeviceInfo,
        catalog: &VirtualHidProfileCatalog,
    ) -> Self {
        let mut nodes = Vec::new();
        let mut seen = Vec::new();
        while let Some(info) = original.as_ref() {
            let profile = if info.path.is_null() {
                None
            } else {
                catalog.profile_for_path(&CStr::from_ptr(info.path).to_string_lossy())
            };
            let replacement = profile.and_then(|profile| {
                let node = OwnedNode::virtual_device(catalog, profile)?;
                seen.push(profile);
                Some(node)
            });
            nodes.push(replacement.unwrap_or_else(|| OwnedNode::copy_native(info)));
            original = info.next;
        }
        for descriptor in catalog.descriptors() {
            if !seen.contains(&descriptor.profile) {
                if let Some(node) = OwnedNode::virtual_device(catalog, descriptor.profile) {
                    nodes.push(node);
                }
            }
        }
        for index in 0..nodes.len() {
            let next = nodes
                .get_mut(index + 1)
                .map_or(ptr::null_mut(), |node| node.info.as_mut() as *mut _);
            nodes[index].info.next = next;
        }
        Self { nodes }
    }

    pub(crate) fn head(&mut self) -> *mut SdlHidDeviceInfo {
        self.nodes
            .first_mut()
            .map_or(ptr::null_mut(), |node| node.info.as_mut() as *mut _)
    }
}

fn wide(value: &str) -> Box<[u16]> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn copy_wide(value: *const u16) -> Option<Box<[u16]>> {
    if value.is_null() {
        return None;
    }
    let mut len = 0;
    while *value.add(len) != 0 {
        len += 1;
    }
    Some(slice::from_raw_parts(value, len + 1).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crosspuck_core::guest_driver::identity::default_fallback_identity;

    fn catalog() -> VirtualHidProfileCatalog {
        VirtualHidProfileCatalog::from_identity(&default_fallback_identity(), true)
    }

    #[test]
    fn synthetic_list_is_linked_and_can_be_repeatedly_owned_and_freed() {
        let catalog = catalog();
        for _ in 0..100 {
            let mut list = unsafe { OwnedSdlEnumeration::augment(ptr::null_mut(), &catalog) };
            let mut head = list.head();
            for descriptor in catalog.descriptors() {
                let info = unsafe { head.as_ref().unwrap() };
                assert_eq!(
                    info.interface_number,
                    c_int::from(descriptor.interface_number)
                );
                assert_eq!(
                    unsafe { CStr::from_ptr(info.path) }.to_str().unwrap(),
                    catalog.device_path(descriptor.profile).unwrap()
                );
                head = info.next;
            }
            assert!(head.is_null());
            // Drop releases every Box and CString, with no raw allocation leaks.
        }
    }

    #[test]
    fn unrelated_device_survives_original_free_with_bytes_and_nulls_preserved() {
        let catalog = catalog();
        let mut original = OwnedNode::new(
            SdlHidDeviceInfo {
                vendor_id: 0x1234,
                product_id: 0x5678,
                interface_class: 3,
                ..SdlHidDeviceInfo::default()
            },
            Some(CString::new(b"unrelated\xff".to_vec()).unwrap()),
            Some(vec![0xd800, 0].into_boxed_slice()),
            None,
            Some(wide("Other pad")),
        );
        let mut copy = unsafe { OwnedSdlEnumeration::augment(original.info.as_mut(), &catalog) };
        assert_eq!(copy.nodes.len(), 1 + catalog.descriptors().len());
        assert_ne!(copy.head(), original.info.as_mut() as *mut _);
        assert_ne!(unsafe { (*copy.head()).path }, original.info.path);
        drop(original);
        let info = unsafe { &*copy.head() };
        assert_eq!(info.vendor_id, 0x1234);
        assert_eq!(info.product_id, 0x5678);
        assert_eq!(info.interface_class, 3);
        assert_eq!(
            unsafe { CStr::from_ptr(info.path) }.to_bytes(),
            b"unrelated\xff"
        );
        assert_eq!(unsafe { *info.serial_number }, 0xd800);
        assert!(info.manufacturer_string.is_null());
    }

    #[test]
    fn matching_native_device_is_replaced_without_mutating_or_duplicating_original() {
        let catalog = catalog();
        let path = r"\\?\hid#vid_28de&pid_1304&mi_03#serial";
        let mut original = OwnedNode::new(
            SdlHidDeviceInfo::default(),
            Some(CString::new(path).unwrap()),
            None,
            None,
            None,
        );
        let mut list = unsafe { OwnedSdlEnumeration::augment(original.info.as_mut(), &catalog) };
        assert_eq!(list.nodes.len(), catalog.descriptors().len());
        assert_eq!(
            unsafe { CStr::from_ptr(original.info.path) }
                .to_str()
                .unwrap(),
            path
        );
        assert!(original.info.next.is_null());
        assert_eq!(original.info.vendor_id, 0);
        assert_eq!(unsafe { (*list.head()).vendor_id }, 0x28de);
        assert_eq!(unsafe { (*list.head()).interface_number }, 3);
    }
}
