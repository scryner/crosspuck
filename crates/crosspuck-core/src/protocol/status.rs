use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum StatusCode {
    Ok = 0,
    BadRequest = 1,
    UnsupportedInterface = 2,
    UnsupportedOperation = 3,
    HidTimeout = 4,
    HidIoError = 5,
    DeviceDisconnected = 6,
    ProtocolError = 7,
    HostBusy = 8,
}

impl StatusCode {
    pub fn is_ok(self) -> bool {
        self == Self::Ok
    }
}

impl TryFrom<u16> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::BadRequest),
            2 => Ok(Self::UnsupportedInterface),
            3 => Ok(Self::UnsupportedOperation),
            4 => Ok(Self::HidTimeout),
            5 => Ok(Self::HidIoError),
            6 => Ok(Self::DeviceDisconnected),
            7 => Ok(Self::ProtocolError),
            8 => Ok(Self::HostBusy),
            other => Err(InvalidStatusCode(other)),
        }
    }
}

impl From<StatusCode> for u16 {
    fn from(value: StatusCode) -> Self {
        value as u16
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Ok => "ok",
            Self::BadRequest => "bad_request",
            Self::UnsupportedInterface => "unsupported_interface",
            Self::UnsupportedOperation => "unsupported_operation",
            Self::HidTimeout => "hid_timeout",
            Self::HidIoError => "hid_io_error",
            Self::DeviceDisconnected => "device_disconnected",
            Self::ProtocolError => "protocol_error",
            Self::HostBusy => "host_busy",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidStatusCode(pub u16);

impl fmt::Display for InvalidStatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid status code: {}", self.0)
    }
}

impl std::error::Error for InvalidStatusCode {}
