use crosspuck_core::hid::{
    open_path_with_new_api, HidCollectionRole, HidDevice, HidSnapshotError, PuckSnapshot,
};
use crosspuck_core::protocol::{
    CollectionRole, FeatureResult, GetFeature, IdentityPayload, SetFeature, SetFeatureResult,
    SetOutput, SetOutputResult, StatusCode, WriteReport, WriteResult,
};
use std::fmt;
use std::time::Duration;

const RUMBLE_STOP: [u8; 10] = [0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
const COMMAND_OFF: [u8; 4] = [0x82, 0x03, 0x00, 0x00];

pub(crate) trait HostBackend: Send + Sync {
    fn identity(&self) -> &IdentityPayload;
    fn input_descriptor(&self) -> Result<InputDescriptor, HostHidError>;
    fn open_input_reader(&self) -> Result<Box<dyn InputReportReader>, HostHidError>;
    fn get_feature(&self, request: &GetFeature) -> FeatureResult;
    fn set_feature(&self, request: &SetFeature) -> SetFeatureResult;
    fn set_output(&self, request: &SetOutput) -> SetOutputResult;
    fn write_report(&self, request: &WriteReport) -> WriteResult;
    fn cleanup_feedback(&self);
}

pub(crate) trait InputReportReader: Send {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HostHidError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InputDescriptor {
    pub interface_number: u8,
    pub role: CollectionRole,
}

#[derive(Clone, Debug)]
pub(crate) struct RealHostBackend {
    snapshot: PuckSnapshot,
    identity: IdentityPayload,
}

impl RealHostBackend {
    pub fn new(snapshot: PuckSnapshot, identity: IdentityPayload) -> Self {
        Self { snapshot, identity }
    }

    fn path_for_interface(&self, interface_number: u8) -> Option<&str> {
        self.snapshot
            .collections
            .iter()
            .find(|collection| collection.interface_number == i32::from(interface_number))
            .map(|collection| collection.path.as_str())
    }

    fn main_collection(&self) -> Result<(&str, u16, InputDescriptor), HostHidError> {
        let collection = self
            .snapshot
            .collections
            .iter()
            .find(|collection| collection.role == HidCollectionRole::PuckMain)
            .ok_or(HostHidError::MissingCollection(HidCollectionRole::PuckMain))?;
        let interface_number = u8::try_from(collection.interface_number)
            .map_err(|_| HostHidError::InvalidInterfaceNumber(collection.interface_number))?;
        Ok((
            collection.path.as_str(),
            collection.input_report_len,
            InputDescriptor {
                interface_number,
                role: CollectionRole::PuckMain,
            },
        ))
    }

    fn write_raw(&self, interface_number: u8, data: &[u8]) -> WriteResult {
        let Some(path) = self.path_for_interface(interface_number) else {
            return WriteResult {
                status: StatusCode::UnsupportedInterface,
                bytes_written: 0,
                os_error: 0,
            };
        };

        match open_path_with_new_api(path).and_then(|device| device.write(data).map_err(Into::into))
        {
            Ok(written) => WriteResult {
                status: StatusCode::Ok,
                bytes_written: saturating_u16(written),
                os_error: 0,
            },
            Err(error) => hid_write_error(error),
        }
    }
}

impl HostBackend for RealHostBackend {
    fn identity(&self) -> &IdentityPayload {
        &self.identity
    }

    fn input_descriptor(&self) -> Result<InputDescriptor, HostHidError> {
        self.main_collection().map(|(_, _, descriptor)| descriptor)
    }

    fn open_input_reader(&self) -> Result<Box<dyn InputReportReader>, HostHidError> {
        let (path, input_report_len, _) = self.main_collection()?;
        let device = open_path_with_new_api(path)?;
        Ok(Box::new(RealInputReportReader {
            device,
            buffer: vec![0_u8; usize::from(input_report_len).max(64)],
        }))
    }

    fn get_feature(&self, request: &GetFeature) -> FeatureResult {
        let Some(path) = self.path_for_interface(request.interface_number) else {
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
        match open_path_with_new_api(path)
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

    fn set_feature(&self, request: &SetFeature) -> SetFeatureResult {
        let Some(path) = self.path_for_interface(request.interface_number) else {
            return SetFeatureResult {
                status: StatusCode::UnsupportedInterface,
                bytes_accepted: 0,
                os_error: 0,
            };
        };

        match open_path_with_new_api(path).and_then(|device| {
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

    fn set_output(&self, request: &SetOutput) -> SetOutputResult {
        let write = self.write_raw(request.interface_number, &request.data);
        SetOutputResult {
            status: write.status,
            bytes_accepted: write.bytes_written,
            os_error: write.os_error,
        }
    }

    fn write_report(&self, request: &WriteReport) -> WriteResult {
        self.write_raw(request.interface_number, &request.data)
    }

    fn cleanup_feedback(&self) {
        let Ok((path, _, _)) = self.main_collection() else {
            return;
        };
        if let Ok(device) = open_path_with_new_api(path) {
            let _ = device.write(&RUMBLE_STOP);
            let _ = device.write(&COMMAND_OFF);
        }
    }
}

struct RealInputReportReader {
    device: HidDevice,
    buffer: Vec<u8>,
}

impl InputReportReader for RealInputReportReader {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HostHidError> {
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        match self
            .device
            .read_timeout(&mut self.buffer, timeout_ms)
            .map_err(HidSnapshotError::from)?
        {
            0 => Ok(None),
            read => Ok(Some(self.buffer[..read].to_vec())),
        }
    }
}

fn saturating_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn hid_write_error(_error: HidSnapshotError) -> WriteResult {
    WriteResult {
        status: StatusCode::HidIoError,
        bytes_written: 0,
        os_error: 0,
    }
}

#[derive(Debug)]
pub(crate) enum HostHidError {
    MissingCollection(HidCollectionRole),
    InvalidInterfaceNumber(i32),
    Hid(HidSnapshotError),
}

impl fmt::Display for HostHidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCollection(role) => write!(f, "missing HID collection: {}", role.label()),
            Self::InvalidInterfaceNumber(interface_number) => {
                write!(f, "invalid HID interface number: {interface_number}")
            }
            Self::Hid(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HostHidError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hid(error) => Some(error),
            _ => None,
        }
    }
}

impl From<HidSnapshotError> for HostHidError {
    fn from(value: HidSnapshotError) -> Self {
        Self::Hid(value)
    }
}
