use std::fmt;

pub const CONTROL_ADDR: &str = "127.0.0.1:28473";
pub const INPUT_ADDR: &str = "127.0.0.1:28474";
pub const PROTOCOL_VERSION: u8 = 1;
pub const FRAME_HEADER_LEN: usize = 12;
pub const MAGIC: [u8; 2] = *b"CP";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MessageType {
    Hello = 0x01,
    HelloOk = 0x02,
    Identity = 0x03,
    InputAttach = 0x04,
    InputAttachOk = 0x05,
    Status = 0x06,
    Ping = 0x07,
    Pong = 0x08,
    GetFeature = 0x10,
    FeatureResult = 0x11,
    SetFeature = 0x12,
    SetFeatureResult = 0x13,
    SetOutput = 0x14,
    SetOutputResult = 0x15,
    Write = 0x16,
    WriteResult = 0x17,
    Ioctl = 0x18,
    IoctlResult = 0x19,
    InputReport = 0x30,
}

impl TryFrom<u8> for MessageType {
    type Error = FrameError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Hello),
            0x02 => Ok(Self::HelloOk),
            0x03 => Ok(Self::Identity),
            0x04 => Ok(Self::InputAttach),
            0x05 => Ok(Self::InputAttachOk),
            0x06 => Ok(Self::Status),
            0x07 => Ok(Self::Ping),
            0x08 => Ok(Self::Pong),
            0x10 => Ok(Self::GetFeature),
            0x11 => Ok(Self::FeatureResult),
            0x12 => Ok(Self::SetFeature),
            0x13 => Ok(Self::SetFeatureResult),
            0x14 => Ok(Self::SetOutput),
            0x15 => Ok(Self::SetOutputResult),
            0x16 => Ok(Self::Write),
            0x17 => Ok(Self::WriteResult),
            0x18 => Ok(Self::Ioctl),
            0x19 => Ok(Self::IoctlResult),
            0x30 => Ok(Self::InputReport),
            other => Err(FrameError::UnknownMessageType(other)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    pub message_type: MessageType,
    pub id: u32,
    pub payload_len: u32,
}

impl FrameHeader {
    pub fn encode(&self) -> [u8; FRAME_HEADER_LEN] {
        let mut header = [0_u8; FRAME_HEADER_LEN];
        header[0..2].copy_from_slice(&MAGIC);
        header[2] = PROTOCOL_VERSION;
        header[3] = self.message_type as u8;
        header[4..8].copy_from_slice(&self.id.to_le_bytes());
        header[8..12].copy_from_slice(&self.payload_len.to_le_bytes());
        header
    }

    pub fn decode(bytes: [u8; FRAME_HEADER_LEN]) -> Result<Self, FrameError> {
        if bytes[0..2] != MAGIC {
            return Err(FrameError::BadMagic([bytes[0], bytes[1]]));
        }
        if bytes[2] != PROTOCOL_VERSION {
            return Err(FrameError::UnsupportedVersion(bytes[2]));
        }

        Ok(Self {
            message_type: MessageType::try_from(bytes[3])?,
            id: u32::from_le_bytes(bytes[4..8].try_into().expect("fixed-size header field")),
            payload_len: u32::from_le_bytes(
                bytes[8..12].try_into().expect("fixed-size header field"),
            ),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(message_type: MessageType, id: u32, payload: Vec<u8>) -> Self {
        Self {
            header: FrameHeader {
                message_type,
                id,
                payload_len: payload.len() as u32,
            },
            payload,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    BadMagic([u8; 2]),
    UnsupportedVersion(u8),
    UnknownMessageType(u8),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic(magic) => write!(f, "bad frame magic: {:02X?}", magic),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported protocol version: {version}")
            }
            Self::UnknownMessageType(message_type) => {
                write!(f, "unknown message type: 0x{message_type:02X}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_header() {
        let header = FrameHeader {
            message_type: MessageType::Identity,
            id: 42,
            payload_len: 128,
        };

        assert_eq!(FrameHeader::decode(header.encode()).unwrap(), header);
    }
}
