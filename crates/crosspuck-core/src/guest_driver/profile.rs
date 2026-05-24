use super::identity::default_fallback_identity;
use crate::protocol::{CollectionDescriptor, CollectionRole, IdentityPayload};

pub const HID_INTERFACE_GUID_STRING: &str = "{4d1e55b2-f16f-11cf-88cb-001111000030}";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VirtualHidProfile {
    Main,
    Interface3,
    Interface4,
    Interface5,
    VendorDongle,
}

impl VirtualHidProfile {
    pub const ALL: [Self; 5] = [
        Self::Main,
        Self::Interface3,
        Self::Interface4,
        Self::Interface5,
        Self::VendorDongle,
    ];

    pub fn role(self) -> CollectionRole {
        match self {
            Self::Main => CollectionRole::PuckMain,
            Self::Interface3 => CollectionRole::PuckInterface3,
            Self::Interface4 => CollectionRole::PuckInterface4,
            Self::Interface5 => CollectionRole::PuckInterface5,
            Self::VendorDongle => CollectionRole::PuckVendorDongle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "puck-if2-main",
            Self::Interface3 => "puck-if3",
            Self::Interface4 => "puck-if4",
            Self::Interface5 => "puck-if5",
            Self::VendorDongle => "puck-vendor-dongle",
        }
    }

    pub fn collection_number(self) -> u8 {
        match self {
            Self::VendorDongle => 2,
            _ => 1,
        }
    }

    pub fn instance_suffix(self) -> u16 {
        match self {
            Self::Main => 1,
            Self::Interface3 => 2,
            Self::Interface4 => 3,
            Self::Interface5 => 4,
            Self::VendorDongle => 5,
        }
    }

    pub fn from_role(role: CollectionRole) -> Self {
        match role {
            CollectionRole::PuckMain => Self::Main,
            CollectionRole::PuckInterface3 => Self::Interface3,
            CollectionRole::PuckInterface4 => Self::Interface4,
            CollectionRole::PuckInterface5 => Self::Interface5,
            CollectionRole::PuckVendorDongle => Self::VendorDongle,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualHidProfileDescriptor {
    pub profile: VirtualHidProfile,
    pub role: CollectionRole,
    pub interface_number: u8,
    pub collection_number: u8,
    pub instance_suffix: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_report_len: u16,
    pub output_report_len: u16,
    pub feature_report_len: u16,
}

impl VirtualHidProfileDescriptor {
    fn from_collection(collection: &CollectionDescriptor) -> Self {
        let profile = VirtualHidProfile::from_role(collection.role);
        Self {
            profile,
            role: collection.role,
            interface_number: collection.interface_number,
            collection_number: profile.collection_number(),
            instance_suffix: profile.instance_suffix(),
            usage_page: collection.usage_page,
            usage: collection.usage,
            input_report_len: collection.input_report_len,
            output_report_len: collection.output_report_len,
            feature_report_len: collection.feature_report_len,
        }
    }

    pub fn caps(&self) -> HidCaps {
        HidCaps {
            usage: self.usage,
            usage_page: self.usage_page,
            input_report_byte_length: self.input_report_len,
            output_report_byte_length: self.output_report_len,
            feature_report_byte_length: self.feature_report_len,
            number_link_collection_nodes: if self.profile == VirtualHidProfile::VendorDongle {
                1
            } else {
                4
            },
            number_input_value_caps: 1,
            number_output_value_caps: u16::from(self.output_report_len > 1),
            number_feature_value_caps: 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HidCaps {
    pub usage: u16,
    pub usage_page: u16,
    pub input_report_byte_length: u16,
    pub output_report_byte_length: u16,
    pub feature_report_byte_length: u16,
    pub number_link_collection_nodes: u16,
    pub number_input_value_caps: u16,
    pub number_output_value_caps: u16,
    pub number_feature_value_caps: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualHidProfileCatalog {
    identity: IdentityPayload,
    descriptors: Vec<VirtualHidProfileDescriptor>,
}

impl VirtualHidProfileCatalog {
    pub fn from_identity(identity: &IdentityPayload, allow_debug_fallback: bool) -> Self {
        let fallback = allow_debug_fallback.then(default_fallback_identity);
        let mut descriptors = Vec::new();

        for profile in VirtualHidProfile::ALL {
            let collection = identity
                .collections
                .iter()
                .find(|collection| collection.role == profile.role())
                .or_else(|| {
                    fallback.as_ref().and_then(|fallback| {
                        fallback
                            .collections
                            .iter()
                            .find(|collection| collection.role == profile.role())
                    })
                });
            if let Some(collection) = collection {
                descriptors.push(VirtualHidProfileDescriptor::from_collection(collection));
            }
        }

        Self {
            identity: identity.clone(),
            descriptors,
        }
    }

    pub fn identity(&self) -> &IdentityPayload {
        &self.identity
    }

    pub fn descriptors(&self) -> &[VirtualHidProfileDescriptor] {
        &self.descriptors
    }

    pub fn descriptor(&self, profile: VirtualHidProfile) -> Option<&VirtualHidProfileDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.profile == profile)
    }

    pub fn profile_for_interface(&self, interface_number: u8) -> Option<VirtualHidProfile> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.interface_number == interface_number)
            .map(|descriptor| descriptor.profile)
    }

    pub fn profile_for_path(&self, path: &str) -> Option<VirtualHidProfile> {
        let lower = path.to_ascii_lowercase();
        self.descriptors.iter().find_map(|descriptor| {
            let mi = format!("mi_{:02x}", descriptor.interface_number);
            let col = format!("col{:02x}", descriptor.collection_number);
            (lower.contains(&mi) && lower.contains(&col)).then_some(descriptor.profile)
        })
    }

    pub fn device_path(&self, profile: VirtualHidProfile) -> Option<String> {
        let descriptor = self.descriptor(profile)?;
        Some(format!(
            r"\\?\hid#vid_{:04x}&pid_{:04x}&{}#{}",
            self.identity.vendor_id,
            self.identity.product_id,
            self.path_instance(descriptor),
            HID_INTERFACE_GUID_STRING
        ))
    }

    pub fn device_instance_id(&self, profile: VirtualHidProfile) -> Option<String> {
        let descriptor = self.descriptor(profile)?;
        Some(format!(
            r"HID\VID_{:04X}&PID_{:04X}&MI_{:02X}&COL{:02X}\{}&0&{:04X}",
            self.identity.vendor_id,
            self.identity.product_id,
            descriptor.interface_number,
            descriptor.collection_number,
            self.identity.serial,
            descriptor.instance_suffix
        ))
    }

    pub fn hardware_ids(&self, profile: VirtualHidProfile) -> Option<Vec<String>> {
        let descriptor = self.descriptor(profile)?;
        Some(vec![
            format!(
                r"HID\VID_{:04X}&PID_{:04X}&MI_{:02X}&COL{:02X}",
                self.identity.vendor_id,
                self.identity.product_id,
                descriptor.interface_number,
                descriptor.collection_number
            ),
            format!(
                r"HID\VID_{:04X}&PID_{:04X}&MI_{:02X}",
                self.identity.vendor_id, self.identity.product_id, descriptor.interface_number
            ),
            format!(
                r"HID\VID_{:04X}&PID_{:04X}",
                self.identity.vendor_id, self.identity.product_id
            ),
            format!(
                r"HID\VID_{:04X}&UP:{:04X}_U:{:04X}",
                self.identity.vendor_id, descriptor.usage_page, descriptor.usage
            ),
            format!(
                r"HID_DEVICE_UP:{:04X}_U:{:04X}",
                descriptor.usage_page, descriptor.usage
            ),
            "HID_DEVICE".to_string(),
        ])
    }

    pub fn compatible_ids(&self, profile: VirtualHidProfile) -> Option<Vec<String>> {
        let descriptor = self.descriptor(profile)?;
        Some(vec![
            format!(
                r"HID_DEVICE_UP:{:04X}_U:{:04X}",
                descriptor.usage_page, descriptor.usage
            ),
            "HID_DEVICE".to_string(),
        ])
    }

    pub fn location_path(&self, profile: VirtualHidProfile) -> Option<String> {
        let descriptor = self.descriptor(profile)?;
        Some(format!(
            r"USBROOT(0)#USB(1)#USBMI({})#HID({})",
            descriptor.interface_number, descriptor.collection_number
        ))
    }

    fn path_instance(&self, descriptor: &VirtualHidProfileDescriptor) -> String {
        format!(
            "mi_{:02x}&col{:02x}#{}&0&{:04x}",
            descriptor.interface_number,
            descriptor.collection_number,
            self.identity.serial.to_ascii_lowercase(),
            descriptor.instance_suffix
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_serial_based_paths_from_identity() {
        let identity = default_fallback_identity();
        let catalog = VirtualHidProfileCatalog::from_identity(&identity, false);

        assert_eq!(
            catalog.device_instance_id(VirtualHidProfile::Main).unwrap(),
            r"HID\VID_28DE&PID_1304&MI_02&COL01\FXB9961303C9C&0&0001"
        );
        assert_eq!(
            catalog
                .device_path(VirtualHidProfile::VendorDongle)
                .unwrap(),
            r"\\?\hid#vid_28de&pid_1304&mi_06&col02#fxb9961303c9c&0&0005#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        );
    }

    #[test]
    fn can_omit_missing_profiles_without_debug_fallback() {
        let mut identity = default_fallback_identity();
        identity
            .collections
            .retain(|collection| collection.role == CollectionRole::PuckMain);

        let catalog = VirtualHidProfileCatalog::from_identity(&identity, false);

        assert_eq!(catalog.descriptors().len(), 1);
        assert!(catalog
            .descriptor(VirtualHidProfile::VendorDongle)
            .is_none());
    }

    #[test]
    fn debug_fallback_fills_missing_profiles() {
        let mut identity = default_fallback_identity();
        identity
            .collections
            .retain(|collection| collection.role == CollectionRole::PuckMain);

        let catalog = VirtualHidProfileCatalog::from_identity(&identity, true);

        assert_eq!(catalog.descriptors().len(), 5);
        assert!(catalog
            .descriptor(VirtualHidProfile::VendorDongle)
            .is_some());
    }
}
