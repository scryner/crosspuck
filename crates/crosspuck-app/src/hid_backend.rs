use crosspuck_core::hid::{
    open_path_with_new_api, HidCollectionInfo, HidCollectionRole, PuckSnapshot,
};
use crosspuck_core::protocol::{
    FeatureResult, GetFeature, SetFeature, SetFeatureResult, SetOutput, SetOutputResult,
    StatusCode, WriteReport, WriteResult,
};
use std::fmt;

const RUMBLE_STOP: [u8; 10] = [0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
const COMMAND_OFF: [u8; 4] = [0x82, 0x03, 0x00, 0x00];

#[derive(Clone, Debug)]
pub struct HostHidBackend {
    snapshot: PuckSnapshot,
}

impl HostHidBackend {
    pub fn new(snapshot: PuckSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn input_collection(&self) -> Result<HidCollectionInfo, HostHidError> {
        self.collection_by_role(HidCollectionRole::PuckMain)
            .cloned()
            .ok_or(HostHidError::MissingCollection(HidCollectionRole::PuckMain))
    }

    pub fn get_feature(&self, request: &GetFeature) -> FeatureResult {
        let Some(collection) = self.collection_for_interface(request.interface_number) else {
            return FeatureResult {
                status: StatusCode::UnsupportedInterface,
                os_error: 0,
                data: Vec::new(),
            };
        };
        if request.requested_len == 0 {
            return FeatureResult {
                status: StatusCode::BadRequest,
                os_error: 0,
                data: Vec::new(),
            };
        }

        let mut buffer = vec![0_u8; request.requested_len as usize];
        buffer[0] = request.report_id;
        match open_path_with_new_api(&collection.path)
            .and_then(|device| device.get_feature_report(&mut buffer).map_err(Into::into))
        {
            Ok(read) => FeatureResult {
                status: StatusCode::Ok,
                os_error: 0,
                data: buffer[..read.min(buffer.len())].to_vec(),
            },
            Err(_) => FeatureResult {
                status: StatusCode::HidIoError,
                os_error: 0,
                data: Vec::new(),
            },
        }
    }

    pub fn set_feature(&self, request: &SetFeature) -> SetFeatureResult {
        let Some(collection) = self.collection_for_interface(request.interface_number) else {
            return SetFeatureResult {
                status: StatusCode::UnsupportedInterface,
                bytes_accepted: 0,
                os_error: 0,
            };
        };

        match open_path_with_new_api(&collection.path).and_then(|device| {
            device
                .send_feature_report(&request.data)
                .map(|()| request.data.len())
                .map_err(Into::into)
        }) {
            Ok(accepted) => SetFeatureResult {
                status: StatusCode::Ok,
                bytes_accepted: saturating_u16(accepted),
                os_error: 0,
            },
            Err(_) => SetFeatureResult {
                status: StatusCode::HidIoError,
                bytes_accepted: 0,
                os_error: 0,
            },
        }
    }

    pub fn set_output(&self, request: &SetOutput) -> SetOutputResult {
        let write = self.write_raw(request.interface_number, &request.data);
        SetOutputResult {
            status: write.status,
            bytes_accepted: write.bytes_written,
            os_error: write.os_error,
        }
    }

    pub fn write_report(&self, request: &WriteReport) -> WriteResult {
        self.write_raw(request.interface_number, &request.data)
    }

    pub fn cleanup_feedback(&self) {
        let Some(collection) = self.collection_by_role(HidCollectionRole::PuckMain) else {
            return;
        };
        if let Ok(device) = open_path_with_new_api(&collection.path) {
            let _ = device.write(&RUMBLE_STOP);
            let _ = device.write(&COMMAND_OFF);
        }
    }

    fn write_raw(&self, interface_number: u8, data: &[u8]) -> WriteResult {
        let Some(collection) = self.collection_for_interface(interface_number) else {
            return WriteResult {
                status: StatusCode::UnsupportedInterface,
                bytes_written: 0,
                os_error: 0,
            };
        };

        match open_path_with_new_api(&collection.path)
            .and_then(|device| device.write(data).map_err(Into::into))
        {
            Ok(written) => WriteResult {
                status: StatusCode::Ok,
                bytes_written: saturating_u16(written),
                os_error: 0,
            },
            Err(_) => WriteResult {
                status: StatusCode::HidIoError,
                bytes_written: 0,
                os_error: 0,
            },
        }
    }

    fn collection_for_interface(&self, interface_number: u8) -> Option<&HidCollectionInfo> {
        self.snapshot
            .collections
            .iter()
            .find(|collection| collection.interface_number == i32::from(interface_number))
    }

    fn collection_by_role(&self, role: HidCollectionRole) -> Option<&HidCollectionInfo> {
        self.snapshot
            .collections
            .iter()
            .find(|collection| collection.role == role)
    }
}

fn saturating_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

#[derive(Debug)]
pub enum HostHidError {
    MissingCollection(HidCollectionRole),
}

impl fmt::Display for HostHidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCollection(role) => write!(f, "missing HID collection: {}", role.label()),
        }
    }
}

impl std::error::Error for HostHidError {}
