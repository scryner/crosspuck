use crate::protocol::{InvalidStatusCode, MessageType};
use std::fmt;

pub trait WireEncode {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError>;

    fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Vec::new();
        self.encode(&mut out)?;
        Ok(out)
    }
}

pub trait WireDecode: Sized {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError>;
}

pub trait WirePayload: WireEncode + WireDecode {
    const MESSAGE_TYPE: MessageType;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    InvalidUtf8,
    InvalidLength(&'static str),
    InvalidReserved(&'static str),
    InvalidStatusCode(u16),
    InvalidCollectionRole(u8),
    InvalidLogSeverity(u8),
    TrailingBytes(usize),
    WrongMessageType {
        expected: MessageType,
        actual: MessageType,
    },
    StringTooLong(usize),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => f.write_str("invalid utf-8 string"),
            Self::InvalidLength(field) => write!(f, "invalid length for {field}"),
            Self::InvalidReserved(field) => write!(f, "reserved field must be zero: {field}"),
            Self::InvalidStatusCode(code) => write!(f, "invalid status code: {code}"),
            Self::InvalidCollectionRole(role) => {
                write!(f, "invalid collection role: {role}")
            }
            Self::InvalidLogSeverity(severity) => {
                write!(f, "invalid log severity: {severity}")
            }
            Self::TrailingBytes(count) => write!(f, "trailing payload bytes: {count}"),
            Self::WrongMessageType { expected, actual } => {
                write!(
                    f,
                    "wrong message type: expected {expected:?}, got {actual:?}"
                )
            }
            Self::StringTooLong(len) => write!(f, "string too long for wire encoding: {len}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<InvalidStatusCode> for ProtocolError {
    fn from(value: InvalidStatusCode) -> Self {
        Self::InvalidStatusCode(value.0)
    }
}

pub(crate) struct Encoder<'a> {
    out: &'a mut Vec<u8>,
}

impl<'a> Encoder<'a> {
    pub(crate) fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out }
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.out.push(value);
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.out.extend_from_slice(value);
    }

    pub(crate) fn string(&mut self, value: &str) -> Result<(), ProtocolError> {
        let len =
            u16::try_from(value.len()).map_err(|_| ProtocolError::StringTooLong(value.len()))?;
        self.u16(len);
        self.bytes(value.as_bytes());
        Ok(())
    }
}

pub(crate) struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    pub(crate) fn finish(&self) -> Result<(), ProtocolError> {
        let trailing = self.input.len().saturating_sub(self.offset);
        if trailing == 0 {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes(trailing))
        }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.offset)
    }

    pub(crate) fn u8(&mut self, field: &'static str) -> Result<u8, ProtocolError> {
        let bytes = self.take(field, 1)?;
        Ok(bytes[0])
    }

    pub(crate) fn u16(&mut self, field: &'static str) -> Result<u16, ProtocolError> {
        let bytes = self.take(field, 2)?;
        Ok(u16::from_le_bytes(bytes.try_into().expect("u16 field")))
    }

    pub(crate) fn u32(&mut self, field: &'static str) -> Result<u32, ProtocolError> {
        let bytes = self.take(field, 4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("u32 field")))
    }

    pub(crate) fn u64(&mut self, field: &'static str) -> Result<u64, ProtocolError> {
        let bytes = self.take(field, 8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("u64 field")))
    }

    pub(crate) fn reserved_u8(&mut self, field: &'static str) -> Result<(), ProtocolError> {
        match self.u8(field)? {
            0 => Ok(()),
            _ => Err(ProtocolError::InvalidReserved(field)),
        }
    }

    pub(crate) fn reserved_u16(&mut self, field: &'static str) -> Result<(), ProtocolError> {
        match self.u16(field)? {
            0 => Ok(()),
            _ => Err(ProtocolError::InvalidReserved(field)),
        }
    }

    pub(crate) fn bytes(
        &mut self,
        field: &'static str,
        len: usize,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(self.take(field, len)?.to_vec())
    }

    pub(crate) fn string(&mut self, field: &'static str) -> Result<String, ProtocolError> {
        let len = self.u16(field)? as usize;
        let bytes = self.take(field, len)?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_owned())
            .map_err(|_| ProtocolError::InvalidUtf8)
    }

    fn take(&mut self, field: &'static str, len: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ProtocolError::InvalidLength(field))?;
        if end > self.input.len() {
            return Err(ProtocolError::InvalidLength(field));
        }
        let bytes = &self.input[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
}
