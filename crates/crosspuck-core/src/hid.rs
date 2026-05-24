use hidapi::{DeviceInfo, HidApi, HidDevice};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fmt;

pub const VALVE_VENDOR_ID: u16 = 0x28DE;
pub const STEAM_CONTROLLER_PUCK_PRODUCT_ID: u16 = 0x1304;
pub const DEFAULT_INPUT_REPORT_LEN: u16 = 54;
pub const DEFAULT_CONTROLLER_OUTPUT_REPORT_LEN: u16 = 64;
pub const DEFAULT_DONGLE_OUTPUT_REPORT_LEN: u16 = 1;
pub const DEFAULT_FEATURE_REPORT_LEN: u16 = 64;

#[derive(Clone, Debug, Default)]
pub struct HidFilter {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub serial: Option<String>,
}

impl HidFilter {
    pub fn steam_puck() -> Self {
        Self {
            vendor_id: Some(VALVE_VENDOR_ID),
            product_id: Some(STEAM_CONTROLLER_PUCK_PRODUCT_ID),
            serial: None,
        }
    }

    pub fn matches(&self, device: &DeviceInfo) -> bool {
        self.vendor_id
            .is_none_or(|vendor_id| device.vendor_id() == vendor_id)
            && self
                .product_id
                .is_none_or(|product_id| device.product_id() == product_id)
            && self.serial.as_ref().is_none_or(|serial| {
                device
                    .serial_number()
                    .is_some_and(|candidate| candidate == serial)
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceCandidate {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
    pub interface_number: i32,
    pub usage_page: u16,
    pub usage: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

impl DeviceCandidate {
    pub fn from_device(device: &DeviceInfo) -> Self {
        Self {
            path: device.path().to_string_lossy().into_owned(),
            vendor_id: device.vendor_id(),
            product_id: device.product_id(),
            version_number: device.release_number(),
            interface_number: device.interface_number(),
            usage_page: device.usage_page(),
            usage: device.usage(),
            manufacturer: device.manufacturer_string().map(ToOwned::to_owned),
            product: device.product_string().map(ToOwned::to_owned),
            serial: device.serial_number().map(ToOwned::to_owned),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PuckIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum HidCollectionRole {
    #[serde(rename = "puck-main")]
    PuckMain,
    #[serde(rename = "puck-interface-3")]
    PuckInterface3,
    #[serde(rename = "puck-interface-4")]
    PuckInterface4,
    #[serde(rename = "puck-interface-5")]
    PuckInterface5,
    #[serde(rename = "puck-vendor-dongle")]
    PuckVendorDongle,
}

impl HidCollectionRole {
    pub fn id(self) -> u8 {
        match self {
            Self::PuckMain => 1,
            Self::PuckInterface3 => 2,
            Self::PuckInterface4 => 3,
            Self::PuckInterface5 => 4,
            Self::PuckVendorDongle => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PuckMain => "puck-main",
            Self::PuckInterface3 => "puck-interface-3",
            Self::PuckInterface4 => "puck-interface-4",
            Self::PuckInterface5 => "puck-interface-5",
            Self::PuckVendorDongle => "puck-vendor-dongle",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HidCollectionInfo {
    pub role: HidCollectionRole,
    pub interface_number: i32,
    pub usage_page: u16,
    pub usage: u16,
    pub input_report_len: u16,
    pub output_report_len: u16,
    pub feature_report_len: u16,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PuckSnapshot {
    pub protocol: u16,
    pub identity: PuckIdentity,
    pub collections: Vec<HidCollectionInfo>,
}

#[derive(Debug)]
pub enum HidSnapshotError {
    NoMatchingDevice,
    MissingSerial,
    MissingCollections,
    InvalidPath,
    Hid(hidapi::HidError),
}

impl fmt::Display for HidSnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMatchingDevice => write!(f, "no matching Steam Controller Puck HID device"),
            Self::MissingSerial => write!(f, "matching Puck has no HID serial string"),
            Self::MissingCollections => write!(f, "matching Puck has no mapped HID collections"),
            Self::InvalidPath => write!(f, "HID path contains an embedded NUL byte"),
            Self::Hid(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HidSnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hid(error) => Some(error),
            _ => None,
        }
    }
}

impl From<hidapi::HidError> for HidSnapshotError {
    fn from(value: hidapi::HidError) -> Self {
        Self::Hid(value)
    }
}

pub fn collect_candidates(api: &HidApi, filter: &HidFilter) -> Vec<DeviceCandidate> {
    api.device_list()
        .filter(|device| filter.matches(device))
        .map(DeviceCandidate::from_device)
        .collect()
}

pub fn unique_by_path(candidates: &[DeviceCandidate]) -> Vec<&DeviceCandidate> {
    let mut unique = Vec::new();
    for candidate in candidates {
        if unique
            .iter()
            .any(|existing: &&DeviceCandidate| existing.path == candidate.path)
        {
            continue;
        }
        unique.push(candidate);
    }
    unique.sort_by_key(|candidate| std::cmp::Reverse(target_score(candidate)));
    unique
}

pub fn choose_target(candidates: &[DeviceCandidate]) -> Option<&DeviceCandidate> {
    candidates
        .iter()
        .max_by_key(|device| target_score(device))
        .or_else(|| candidates.first())
}

pub fn target_score(device: &DeviceCandidate) -> u16 {
    let observed_puck_report_stream =
        u16::from(device.interface_number == 2 && device.usage_page == 0x0001) * 200;
    let generic_desktop_mouse =
        u16::from(device.usage_page == 0x0001 && device.usage == 0x0002) * 75;
    let vendor_specific = u16::from(device.usage_page == 0xFF00) * 50;
    let raw_channel = u16::from(device.usage_page == 0xFF00 && device.usage == 0x0002) * 25;

    observed_puck_report_stream + generic_desktop_mouse + vendor_specific + raw_channel
}

pub fn open_device(api: &HidApi, target: &DeviceCandidate) -> Result<HidDevice, HidSnapshotError> {
    let path = CString::new(target.path.as_bytes()).map_err(|_| HidSnapshotError::InvalidPath)?;
    api.open_path(path.as_c_str()).map_err(Into::into)
}

pub fn build_puck_snapshot(
    candidates: &[DeviceCandidate],
) -> Result<PuckSnapshot, HidSnapshotError> {
    let identity_source = candidates
        .iter()
        .find(|candidate| {
            candidate.vendor_id == VALVE_VENDOR_ID
                && candidate.product_id == STEAM_CONTROLLER_PUCK_PRODUCT_ID
                && candidate
                    .serial
                    .as_deref()
                    .is_some_and(|serial| !serial.is_empty())
        })
        .or_else(|| candidates.first())
        .ok_or(HidSnapshotError::NoMatchingDevice)?;

    let serial = identity_source
        .serial
        .clone()
        .filter(|serial| !serial.is_empty())
        .ok_or(HidSnapshotError::MissingSerial)?;

    let identity = PuckIdentity {
        vendor_id: identity_source.vendor_id,
        product_id: identity_source.product_id,
        version_number: identity_source.version_number,
        manufacturer: identity_source
            .manufacturer
            .clone()
            .unwrap_or_else(|| "Valve Software".to_string()),
        product: identity_source
            .product
            .clone()
            .unwrap_or_else(|| "Steam Controller Puck".to_string()),
        serial,
    };

    let mut collections_by_role = BTreeMap::<HidCollectionRole, HidCollectionInfo>::new();
    for candidate in candidates {
        if candidate.vendor_id != identity.vendor_id || candidate.product_id != identity.product_id
        {
            continue;
        }

        let Some(collection) = collection_info(candidate) else {
            continue;
        };
        collections_by_role
            .entry(collection.role)
            .or_insert(collection);
    }

    let collections = collections_by_role.into_values().collect::<Vec<_>>();
    if collections.is_empty() {
        return Err(HidSnapshotError::MissingCollections);
    }

    Ok(PuckSnapshot {
        protocol: 1,
        identity,
        collections,
    })
}

pub fn snapshot_for_filter(filter: &HidFilter) -> Result<PuckSnapshot, HidSnapshotError> {
    let api = HidApi::new()?;
    let candidates = collect_candidates(&api, filter);
    build_puck_snapshot(&candidates)
}

fn collection_info(candidate: &DeviceCandidate) -> Option<HidCollectionInfo> {
    let role = role_for_candidate(candidate)?;
    let (output_report_len, feature_report_len) = match role {
        HidCollectionRole::PuckVendorDongle => {
            (DEFAULT_DONGLE_OUTPUT_REPORT_LEN, DEFAULT_FEATURE_REPORT_LEN)
        }
        _ => (
            DEFAULT_CONTROLLER_OUTPUT_REPORT_LEN,
            DEFAULT_FEATURE_REPORT_LEN,
        ),
    };

    Some(HidCollectionInfo {
        role,
        interface_number: candidate.interface_number,
        usage_page: candidate.usage_page,
        usage: candidate.usage,
        input_report_len: DEFAULT_INPUT_REPORT_LEN,
        output_report_len,
        feature_report_len,
        path: candidate.path.clone(),
    })
}

fn role_for_candidate(candidate: &DeviceCandidate) -> Option<HidCollectionRole> {
    match (
        candidate.interface_number,
        candidate.usage_page,
        candidate.usage,
    ) {
        (2, 0x0001, 0x0001) | (2, 0x0001, 0x0002) | (2, 0xFF00, 0x0001) => {
            Some(HidCollectionRole::PuckMain)
        }
        (3, 0x0001, 0x0001) | (3, 0xFF00, 0x0001) => Some(HidCollectionRole::PuckInterface3),
        (4, 0x0001, 0x0001) | (4, 0xFF00, 0x0001) => Some(HidCollectionRole::PuckInterface4),
        (5, 0x0001, 0x0001) | (5, 0xFF00, 0x0001) => Some(HidCollectionRole::PuckInterface5),
        (6, 0xFF00, 0x0002) => Some(HidCollectionRole::PuckVendorDongle),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(interface_number: i32, usage_page: u16, usage: u16) -> DeviceCandidate {
        DeviceCandidate {
            path: format!("test-interface-{interface_number}-{usage_page:04x}-{usage:04x}"),
            vendor_id: VALVE_VENDOR_ID,
            product_id: STEAM_CONTROLLER_PUCK_PRODUCT_ID,
            version_number: 2,
            interface_number,
            usage_page,
            usage,
            manufacturer: Some("Valve Software".to_string()),
            product: Some("Steam Controller Puck".to_string()),
            serial: Some("FXB9961303C9C".to_string()),
        }
    }

    #[test]
    fn maps_interface_two_mouse_usage_to_puck_main() {
        assert_eq!(
            role_for_candidate(&candidate(2, 0x0001, 0x0002)),
            Some(HidCollectionRole::PuckMain)
        );
    }

    #[test]
    fn maps_vendor_dongle_collection() {
        assert_eq!(
            role_for_candidate(&candidate(6, 0xFF00, 0x0002)),
            Some(HidCollectionRole::PuckVendorDongle)
        );
    }
}
