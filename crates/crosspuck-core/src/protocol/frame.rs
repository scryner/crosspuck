use std::fmt;
use std::io::{self, Read, Write};

pub const CONTROL_ADDR: &str = "127.0.0.1:28473";
pub const INPUT_ADDR: &str = "127.0.0.1:28474";
pub const PROTOCOL_VERSION: u8 = 1;
pub const FRAME_HEADER_LEN: usize = 12;
pub const CONTROL_PAYLOAD_LIMIT: usize = 4096;
pub const INPUT_PAYLOAD_LIMIT: usize = 256;
pub const MAGIC: [u8; 2] = *b"CP";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    Control,
    Input,
}

impl Channel {
    pub fn payload_limit(self) -> usize {
        match self {
            Self::Control => CONTROL_PAYLOAD_LIMIT,
            Self::Input => INPUT_PAYLOAD_LIMIT,
        }
    }
}

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

pub fn read_frame<R: Read>(reader: &mut R, channel: Channel) -> Result<Frame, FrameIoError> {
    let mut header_bytes = [0_u8; FRAME_HEADER_LEN];
    reader.read_exact(&mut header_bytes)?;

    let header = FrameHeader::decode(header_bytes)?;
    let payload_len =
        usize::try_from(header.payload_len).map_err(|_| FrameIoError::PayloadLenOverflow {
            payload_len: header.payload_len,
        })?;
    let limit = channel.payload_limit();
    if payload_len > limit {
        return Err(FrameIoError::PayloadTooLarge {
            channel,
            payload_len,
            limit,
        });
    }

    let mut payload = vec![0_u8; payload_len];
    reader.read_exact(&mut payload)?;

    Ok(Frame { header, payload })
}

pub fn write_frame<W: Write>(
    writer: &mut W,
    frame: &Frame,
    channel: Channel,
) -> Result<(), FrameIoError> {
    let actual_len = frame.payload.len();
    if frame.header.payload_len as usize != actual_len {
        return Err(FrameIoError::PayloadLenMismatch {
            declared: frame.header.payload_len,
            actual: actual_len,
        });
    }

    let limit = channel.payload_limit();
    if actual_len > limit {
        return Err(FrameIoError::PayloadTooLarge {
            channel,
            payload_len: actual_len,
            limit,
        });
    }

    writer.write_all(&frame.header.encode())?;
    writer.write_all(&frame.payload)?;
    writer.flush()?;
    Ok(())
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

#[derive(Debug)]
pub enum FrameIoError {
    Io(io::Error),
    Frame(FrameError),
    PayloadTooLarge {
        channel: Channel,
        payload_len: usize,
        limit: usize,
    },
    PayloadLenMismatch {
        declared: u32,
        actual: usize,
    },
    PayloadLenOverflow {
        payload_len: u32,
    },
}

impl fmt::Display for FrameIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Frame(error) => write!(f, "{error}"),
            Self::PayloadTooLarge {
                channel,
                payload_len,
                limit,
            } => write!(
                f,
                "{channel:?} frame payload too large: {payload_len} > {limit}"
            ),
            Self::PayloadLenMismatch { declared, actual } => write!(
                f,
                "frame payload length mismatch: declared {declared}, actual {actual}"
            ),
            Self::PayloadLenOverflow { payload_len } => {
                write!(f, "frame payload length cannot fit usize: {payload_len}")
            }
        }
    }
}

impl std::error::Error for FrameIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for FrameIoError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<FrameError> for FrameIoError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

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

    #[test]
    fn write_frame_preserves_opaque_payload_bytes() {
        let payload = vec![0x00, 0x80, 0xAA, 0xBB, 0x00];
        let frame = Frame::new(MessageType::Write, 7, payload.clone());

        assert_eq!(frame.header.message_type, MessageType::Write);
        assert_eq!(frame.header.payload_len, payload.len() as u32);
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn set_output_frame_preserves_short_opaque_payload() {
        let payload = vec![0x80, 0x00, 0x00, 0x00, 0x34, 0x12, 0x00, 0x78, 0x56, 0x00];
        let frame = Frame::new(MessageType::SetOutput, 8, payload.clone());

        assert_eq!(frame.header.message_type, MessageType::SetOutput);
        assert_eq!(frame.header.payload_len, 10);
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn read_and_write_frame_round_trip() {
        let frame = Frame::new(MessageType::Hello, 1, vec![0x44, 0x33, 0x22, 0x11]);
        let mut bytes = Vec::new();

        write_frame(&mut bytes, &frame, Channel::Control).unwrap();
        let decoded = read_frame(&mut bytes.as_slice(), Channel::Control).unwrap();

        assert_eq!(decoded, frame);
    }

    #[test]
    fn input_channel_rejects_oversized_payload_before_write() {
        let frame = Frame::new(
            MessageType::InputReport,
            1,
            vec![0_u8; INPUT_PAYLOAD_LIMIT + 1],
        );
        let mut bytes = Vec::new();

        assert!(matches!(
            write_frame(&mut bytes, &frame, Channel::Input),
            Err(FrameIoError::PayloadTooLarge {
                channel: Channel::Input,
                payload_len,
                limit: INPUT_PAYLOAD_LIMIT,
            }) if payload_len == INPUT_PAYLOAD_LIMIT + 1
        ));
        assert!(bytes.is_empty());
    }

    #[test]
    fn control_channel_accepts_payload_at_limit() {
        let frame = Frame::new(MessageType::Write, 9, vec![0xA5; CONTROL_PAYLOAD_LIMIT]);
        let mut bytes = Vec::new();

        write_frame(&mut bytes, &frame, Channel::Control).unwrap();
        let decoded = read_frame(&mut bytes.as_slice(), Channel::Control).unwrap();

        assert_eq!(decoded.payload, frame.payload);
    }

    #[test]
    fn read_frame_rejects_oversized_header_before_payload_read() {
        let header = FrameHeader {
            message_type: MessageType::InputReport,
            id: 99,
            payload_len: (INPUT_PAYLOAD_LIMIT + 1) as u32,
        };
        let bytes = header.encode().to_vec();

        assert!(matches!(
            read_frame(&mut bytes.as_slice(), Channel::Input),
            Err(FrameIoError::PayloadTooLarge {
                channel: Channel::Input,
                payload_len,
                limit: INPUT_PAYLOAD_LIMIT,
            }) if payload_len == INPUT_PAYLOAD_LIMIT + 1
        ));
    }
}
