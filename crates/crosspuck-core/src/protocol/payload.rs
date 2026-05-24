use super::codec::{Decoder, Encoder, ProtocolError, WireDecode, WireEncode, WirePayload};
use super::status::StatusCode;
use super::{MessageType, CONTROL_PAYLOAD_LIMIT, INPUT_PAYLOAD_LIMIT, PROTOCOL_VERSION};

pub const DEFAULT_GUEST_CAPABILITIES: u32 = 0;
pub const DEFAULT_INPUT_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hello {
    pub guest_pid: u32,
    pub guest_protocol_version: u16,
    pub guest_capabilities: u32,
}

impl Hello {
    pub fn new(guest_pid: u32) -> Self {
        Self {
            guest_pid,
            guest_protocol_version: PROTOCOL_VERSION as u16,
            guest_capabilities: DEFAULT_GUEST_CAPABILITIES,
        }
    }
}

impl WireEncode for Hello {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut encoder = Encoder::new(out);
        encoder.u32(self.guest_pid);
        encoder.u16(self.guest_protocol_version);
        encoder.u16(0);
        encoder.u32(self.guest_capabilities);
        Ok(())
    }
}

impl WireDecode for Hello {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            guest_pid: decoder.u32("guest_pid")?,
            guest_protocol_version: decoder.u16("guest_protocol_version")?,
            guest_capabilities: {
                decoder.reserved_u16("hello.reserved_zero")?;
                decoder.u32("guest_capabilities")?
            },
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl WirePayload for Hello {
    const MESSAGE_TYPE: MessageType = MessageType::Hello;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelloOk {
    pub status: StatusCode,
    pub protocol_version: u16,
    pub session_id: u32,
    pub control_payload_limit: u16,
    pub input_payload_limit: u16,
    pub default_input_report_len: u16,
}

impl HelloOk {
    pub fn success(session_id: u32, default_input_report_len: u16) -> Self {
        Self {
            status: StatusCode::Ok,
            protocol_version: PROTOCOL_VERSION as u16,
            session_id,
            control_payload_limit: CONTROL_PAYLOAD_LIMIT as u16,
            input_payload_limit: INPUT_PAYLOAD_LIMIT as u16,
            default_input_report_len,
        }
    }
}

impl WireEncode for HelloOk {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut encoder = Encoder::new(out);
        encoder.u16(self.status.into());
        encoder.u16(self.protocol_version);
        encoder.u32(self.session_id);
        encoder.u16(self.control_payload_limit);
        encoder.u16(self.input_payload_limit);
        encoder.u16(self.default_input_report_len);
        encoder.u16(0);
        Ok(())
    }
}

impl WireDecode for HelloOk {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            status: StatusCode::try_from(decoder.u16("status")?)?,
            protocol_version: decoder.u16("protocol_version")?,
            session_id: decoder.u32("session_id")?,
            control_payload_limit: decoder.u16("control_payload_limit")?,
            input_payload_limit: decoder.u16("input_payload_limit")?,
            default_input_report_len: decoder.u16("default_input_report_len")?,
        };
        decoder.reserved_u16("hello_ok.reserved_zero")?;
        decoder.finish()?;
        Ok(value)
    }
}

impl WirePayload for HelloOk {
    const MESSAGE_TYPE: MessageType = MessageType::HelloOk;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CollectionRole {
    PuckMain = 1,
    PuckInterface3 = 2,
    PuckInterface4 = 3,
    PuckInterface5 = 4,
    PuckVendorDongle = 5,
}

impl CollectionRole {
    pub fn id(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for CollectionRole {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::PuckMain),
            2 => Ok(Self::PuckInterface3),
            3 => Ok(Self::PuckInterface4),
            4 => Ok(Self::PuckInterface5),
            5 => Ok(Self::PuckVendorDongle),
            other => Err(ProtocolError::InvalidCollectionRole(other)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionDescriptor {
    pub role: CollectionRole,
    pub interface_number: u8,
    pub usage_page: u16,
    pub usage: u16,
    pub input_report_len: u16,
    pub output_report_len: u16,
    pub feature_report_len: u16,
}

impl CollectionDescriptor {
    fn encode(&self, encoder: &mut Encoder<'_>) {
        encoder.u8(self.role.id());
        encoder.u8(self.interface_number);
        encoder.u16(self.usage_page);
        encoder.u16(self.usage);
        encoder.u16(self.input_report_len);
        encoder.u16(self.output_report_len);
        encoder.u16(self.feature_report_len);
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Ok(Self {
            role: CollectionRole::try_from(decoder.u8("collection.role")?)?,
            interface_number: decoder.u8("collection.interface_number")?,
            usage_page: decoder.u16("collection.usage_page")?,
            usage: decoder.u16("collection.usage")?,
            input_report_len: decoder.u16("collection.input_report_len")?,
            output_report_len: decoder.u16("collection.output_report_len")?,
            feature_report_len: decoder.u16("collection.feature_report_len")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityPayload {
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub collections: Vec<CollectionDescriptor>,
}

impl IdentityPayload {
    pub fn default_input_report_len(&self) -> u16 {
        self.collections
            .iter()
            .find(|collection| collection.role == CollectionRole::PuckMain)
            .or_else(|| self.collections.first())
            .map(|collection| collection.input_report_len)
            .unwrap_or(0)
    }
}

impl WireEncode for IdentityPayload {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let collection_count = u8::try_from(self.collections.len())
            .map_err(|_| ProtocolError::InvalidLength("collection_count"))?;
        if collection_count == 0 || collection_count > 16 {
            return Err(ProtocolError::InvalidLength("collection_count"));
        }

        let mut encoder = Encoder::new(out);
        encoder.u16(self.vendor_id);
        encoder.u16(self.product_id);
        encoder.u16(self.version_number);
        encoder.string(&self.manufacturer)?;
        encoder.string(&self.product)?;
        encoder.string(&self.serial)?;
        encoder.u8(collection_count);
        for collection in &self.collections {
            collection.encode(&mut encoder);
        }
        Ok(())
    }
}

impl WireDecode for IdentityPayload {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let vendor_id = decoder.u16("vendor_id")?;
        let product_id = decoder.u16("product_id")?;
        let version_number = decoder.u16("version_number")?;
        let manufacturer = decoder.string("manufacturer")?;
        let product = decoder.string("product")?;
        let serial = decoder.string("serial")?;
        let collection_count = decoder.u8("collection_count")?;
        if collection_count == 0 || collection_count > 16 {
            return Err(ProtocolError::InvalidLength("collection_count"));
        }

        let mut collections = Vec::with_capacity(collection_count as usize);
        for _ in 0..collection_count {
            collections.push(CollectionDescriptor::decode(&mut decoder)?);
        }
        decoder.finish()?;
        Ok(Self {
            vendor_id,
            product_id,
            version_number,
            manufacturer,
            product,
            serial,
            collections,
        })
    }
}

impl WirePayload for IdentityPayload {
    const MESSAGE_TYPE: MessageType = MessageType::Identity;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputAttach {
    pub session_id: u32,
    pub last_seen_input_seq: u32,
}

impl WireEncode for InputAttach {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut encoder = Encoder::new(out);
        encoder.u32(self.session_id);
        encoder.u32(self.last_seen_input_seq);
        Ok(())
    }
}

impl WireDecode for InputAttach {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            session_id: decoder.u32("session_id")?,
            last_seen_input_seq: decoder.u32("last_seen_input_seq")?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl WirePayload for InputAttach {
    const MESSAGE_TYPE: MessageType = MessageType::InputAttach;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputAttachOk {
    pub status: StatusCode,
    pub input_report_len: u16,
    pub first_input_seq: u32,
}

impl WireEncode for InputAttachOk {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut encoder = Encoder::new(out);
        encoder.u16(self.status.into());
        encoder.u16(self.input_report_len);
        encoder.u32(self.first_input_seq);
        Ok(())
    }
}

impl WireDecode for InputAttachOk {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            status: StatusCode::try_from(decoder.u16("status")?)?,
            input_report_len: decoder.u16("input_report_len")?,
            first_input_seq: decoder.u32("first_input_seq")?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl WirePayload for InputAttachOk {
    const MESSAGE_TYPE: MessageType = MessageType::InputAttachOk;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GetFeature {
    pub interface_number: u8,
    pub report_id: u8,
    pub requested_len: u16,
    pub timeout_ms: u16,
}

impl WireEncode for GetFeature {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut encoder = Encoder::new(out);
        encoder.u8(self.interface_number);
        encoder.u8(self.report_id);
        encoder.u16(self.requested_len);
        encoder.u16(self.timeout_ms);
        encoder.u16(0);
        Ok(())
    }
}

impl WireDecode for GetFeature {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            interface_number: decoder.u8("interface_number")?,
            report_id: decoder.u8("report_id")?,
            requested_len: decoder.u16("requested_len")?,
            timeout_ms: decoder.u16("timeout_ms")?,
        };
        decoder.reserved_u16("get_feature.reserved_zero")?;
        decoder.finish()?;
        Ok(value)
    }
}

impl WirePayload for GetFeature {
    const MESSAGE_TYPE: MessageType = MessageType::GetFeature;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureResult {
    pub status: StatusCode,
    pub os_error: u32,
    pub data: Vec<u8>,
}

impl WireEncode for FeatureResult {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let data_len =
            u16::try_from(self.data.len()).map_err(|_| ProtocolError::InvalidLength("data_len"))?;
        let mut encoder = Encoder::new(out);
        encoder.u16(self.status.into());
        encoder.u16(data_len);
        encoder.u32(self.os_error);
        encoder.bytes(&self.data);
        Ok(())
    }
}

impl WireDecode for FeatureResult {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let status = StatusCode::try_from(decoder.u16("status")?)?;
        let data_len = decoder.u16("data_len")? as usize;
        let os_error = decoder.u32("os_error")?;
        let data = decoder.bytes("raw_feature_report", data_len)?;
        decoder.finish()?;
        Ok(Self {
            status,
            os_error,
            data,
        })
    }
}

impl WirePayload for FeatureResult {
    const MESSAGE_TYPE: MessageType = MessageType::FeatureResult;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetFeature {
    pub interface_number: u8,
    pub timeout_ms: u16,
    pub data: Vec<u8>,
}

impl WireEncode for SetFeature {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        encode_data_command(out, self.interface_number, self.timeout_ms, &self.data)
    }
}

impl WireDecode for SetFeature {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let (interface_number, timeout_ms, data) = decode_data_command(input, "set_feature.data")?;
        Ok(Self {
            interface_number,
            timeout_ms,
            data,
        })
    }
}

impl WirePayload for SetFeature {
    const MESSAGE_TYPE: MessageType = MessageType::SetFeature;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetFeatureResult {
    pub status: StatusCode,
    pub bytes_accepted: u16,
    pub os_error: u32,
}

impl WireEncode for SetFeatureResult {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        encode_count_result(out, self.status, self.bytes_accepted, self.os_error);
        Ok(())
    }
}

impl WireDecode for SetFeatureResult {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let (status, bytes_accepted, os_error) = decode_count_result(input)?;
        Ok(Self {
            status,
            bytes_accepted,
            os_error,
        })
    }
}

impl WirePayload for SetFeatureResult {
    const MESSAGE_TYPE: MessageType = MessageType::SetFeatureResult;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetOutput {
    pub interface_number: u8,
    pub timeout_ms: u16,
    pub data: Vec<u8>,
}

impl WireEncode for SetOutput {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        encode_data_command(out, self.interface_number, self.timeout_ms, &self.data)
    }
}

impl WireDecode for SetOutput {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let (interface_number, timeout_ms, data) = decode_data_command(input, "set_output.data")?;
        Ok(Self {
            interface_number,
            timeout_ms,
            data,
        })
    }
}

impl WirePayload for SetOutput {
    const MESSAGE_TYPE: MessageType = MessageType::SetOutput;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetOutputResult {
    pub status: StatusCode,
    pub bytes_accepted: u16,
    pub os_error: u32,
}

impl WireEncode for SetOutputResult {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        encode_count_result(out, self.status, self.bytes_accepted, self.os_error);
        Ok(())
    }
}

impl WireDecode for SetOutputResult {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let (status, bytes_accepted, os_error) = decode_count_result(input)?;
        Ok(Self {
            status,
            bytes_accepted,
            os_error,
        })
    }
}

impl WirePayload for SetOutputResult {
    const MESSAGE_TYPE: MessageType = MessageType::SetOutputResult;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteReport {
    pub interface_number: u8,
    pub timeout_ms: u16,
    pub data: Vec<u8>,
}

impl WireEncode for WriteReport {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        encode_data_command(out, self.interface_number, self.timeout_ms, &self.data)
    }
}

impl WireDecode for WriteReport {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let (interface_number, timeout_ms, data) = decode_data_command(input, "write.data")?;
        Ok(Self {
            interface_number,
            timeout_ms,
            data,
        })
    }
}

impl WirePayload for WriteReport {
    const MESSAGE_TYPE: MessageType = MessageType::Write;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteResult {
    pub status: StatusCode,
    pub bytes_written: u16,
    pub os_error: u32,
}

impl WireEncode for WriteResult {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        encode_count_result(out, self.status, self.bytes_written, self.os_error);
        Ok(())
    }
}

impl WireDecode for WriteResult {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let (status, bytes_written, os_error) = decode_count_result(input)?;
        Ok(Self {
            status,
            bytes_written,
            os_error,
        })
    }
}

impl WirePayload for WriteResult {
    const MESSAGE_TYPE: MessageType = MessageType::WriteResult;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IoctlCommand {
    pub ioctl_code: u32,
    pub interface_number: u8,
    pub timeout_ms: u16,
    pub out_len: u16,
    pub data: Vec<u8>,
}

impl WireEncode for IoctlCommand {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let data_len =
            u16::try_from(self.data.len()).map_err(|_| ProtocolError::InvalidLength("in_len"))?;
        let mut encoder = Encoder::new(out);
        encoder.u32(self.ioctl_code);
        encoder.u8(self.interface_number);
        encoder.u8(0);
        encoder.u16(self.timeout_ms);
        encoder.u16(data_len);
        encoder.u16(self.out_len);
        encoder.bytes(&self.data);
        Ok(())
    }
}

impl WireDecode for IoctlCommand {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let ioctl_code = decoder.u32("ioctl_code")?;
        let interface_number = decoder.u8("interface_number")?;
        decoder.reserved_u8("ioctl.reserved_zero")?;
        let timeout_ms = decoder.u16("timeout_ms")?;
        let in_len = decoder.u16("in_len")? as usize;
        let out_len = decoder.u16("out_len")?;
        let data = decoder.bytes("raw_in_buffer", in_len)?;
        decoder.finish()?;
        Ok(Self {
            ioctl_code,
            interface_number,
            timeout_ms,
            out_len,
            data,
        })
    }
}

impl WirePayload for IoctlCommand {
    const MESSAGE_TYPE: MessageType = MessageType::Ioctl;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IoctlResult {
    pub status: StatusCode,
    pub bytes_returned: u32,
    pub os_error: u32,
    pub data: Vec<u8>,
}

impl WireEncode for IoctlResult {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let out_len =
            u16::try_from(self.data.len()).map_err(|_| ProtocolError::InvalidLength("out_len"))?;
        let mut encoder = Encoder::new(out);
        encoder.u16(self.status.into());
        encoder.u16(out_len);
        encoder.u32(self.bytes_returned);
        encoder.u32(self.os_error);
        encoder.bytes(&self.data);
        Ok(())
    }
}

impl WireDecode for IoctlResult {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let status = StatusCode::try_from(decoder.u16("status")?)?;
        let out_len = decoder.u16("out_len")? as usize;
        let bytes_returned = decoder.u32("bytes_returned")?;
        let os_error = decoder.u32("os_error")?;
        let data = decoder.bytes("raw_out_buffer", out_len)?;
        decoder.finish()?;
        Ok(Self {
            status,
            bytes_returned,
            os_error,
            data,
        })
    }
}

impl WirePayload for IoctlResult {
    const MESSAGE_TYPE: MessageType = MessageType::IoctlResult;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputReport {
    pub interface_number: u8,
    pub role: CollectionRole,
    pub host_monotonic_us: u64,
    pub data: Vec<u8>,
}

impl WireEncode for InputReport {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let report_len = u16::try_from(self.data.len())
            .map_err(|_| ProtocolError::InvalidLength("report_len"))?;
        let mut encoder = Encoder::new(out);
        encoder.u8(self.interface_number);
        encoder.u8(self.role.id());
        encoder.u16(report_len);
        encoder.u64(self.host_monotonic_us);
        encoder.bytes(&self.data);
        Ok(())
    }
}

impl WireDecode for InputReport {
    fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(input);
        let interface_number = decoder.u8("interface_number")?;
        let role = CollectionRole::try_from(decoder.u8("role")?)?;
        let report_len = decoder.u16("report_len")? as usize;
        let host_monotonic_us = decoder.u64("host_monotonic_us")?;
        let data = decoder.bytes("raw_input_report", report_len)?;
        decoder.finish()?;
        Ok(Self {
            interface_number,
            role,
            host_monotonic_us,
            data,
        })
    }
}

impl WirePayload for InputReport {
    const MESSAGE_TYPE: MessageType = MessageType::InputReport;
}

fn encode_data_command(
    out: &mut Vec<u8>,
    interface_number: u8,
    timeout_ms: u16,
    data: &[u8],
) -> Result<(), ProtocolError> {
    let data_len =
        u16::try_from(data.len()).map_err(|_| ProtocolError::InvalidLength("data_len"))?;
    let mut encoder = Encoder::new(out);
    encoder.u8(interface_number);
    encoder.u8(0);
    encoder.u16(timeout_ms);
    encoder.u16(data_len);
    encoder.u16(0);
    encoder.bytes(data);
    Ok(())
}

fn decode_data_command(
    input: &[u8],
    data_field: &'static str,
) -> Result<(u8, u16, Vec<u8>), ProtocolError> {
    let mut decoder = Decoder::new(input);
    let interface_number = decoder.u8("interface_number")?;
    decoder.reserved_u8("data_command.reserved_zero")?;
    let timeout_ms = decoder.u16("timeout_ms")?;
    let data_len = decoder.u16("data_len")? as usize;
    decoder.reserved_u16("data_command.reserved_zero")?;
    let data = decoder.bytes(data_field, data_len)?;
    decoder.finish()?;
    Ok((interface_number, timeout_ms, data))
}

fn encode_count_result(out: &mut Vec<u8>, status: StatusCode, count: u16, os_error: u32) {
    let mut encoder = Encoder::new(out);
    encoder.u16(status.into());
    encoder.u16(count);
    encoder.u32(os_error);
}

fn decode_count_result(input: &[u8]) -> Result<(StatusCode, u16, u32), ProtocolError> {
    let mut decoder = Decoder::new(input);
    let status = StatusCode::try_from(decoder.u16("status")?)?;
    let count = decoder.u16("bytes")?;
    let os_error = decoder.u32("os_error")?;
    decoder.finish()?;
    Ok((status, count, os_error))
}

#[cfg(feature = "host-hid")]
impl TryFrom<&crate::hid::PuckSnapshot> for IdentityPayload {
    type Error = ProtocolError;

    fn try_from(snapshot: &crate::hid::PuckSnapshot) -> Result<Self, Self::Error> {
        let mut collections = Vec::with_capacity(snapshot.collections.len());
        for collection in &snapshot.collections {
            collections.push(CollectionDescriptor {
                role: collection.role.into(),
                interface_number: u8::try_from(collection.interface_number)
                    .map_err(|_| ProtocolError::InvalidLength("interface_number"))?,
                usage_page: collection.usage_page,
                usage: collection.usage,
                input_report_len: collection.input_report_len,
                output_report_len: collection.output_report_len,
                feature_report_len: collection.feature_report_len,
            });
        }

        Ok(Self {
            vendor_id: snapshot.identity.vendor_id,
            product_id: snapshot.identity.product_id,
            version_number: snapshot.identity.version_number,
            manufacturer: snapshot.identity.manufacturer.clone(),
            product: snapshot.identity.product.clone(),
            serial: snapshot.identity.serial.clone(),
            collections,
        })
    }
}

#[cfg(feature = "host-hid")]
impl From<crate::hid::HidCollectionRole> for CollectionRole {
    fn from(value: crate::hid::HidCollectionRole) -> Self {
        match value {
            crate::hid::HidCollectionRole::PuckMain => Self::PuckMain,
            crate::hid::HidCollectionRole::PuckInterface3 => Self::PuckInterface3,
            crate::hid::HidCollectionRole::PuckInterface4 => Self::PuckInterface4,
            crate::hid::HidCollectionRole::PuckInterface5 => Self::PuckInterface5,
            crate::hid::HidCollectionRole::PuckVendorDongle => Self::PuckVendorDongle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_matches_golden_vector() {
        let hello = Hello::new(0x1122_3344);

        assert_eq!(
            hello.to_bytes().unwrap(),
            vec![0x44, 0x33, 0x22, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(Hello::decode(&hello.to_bytes().unwrap()).unwrap(), hello);
    }

    #[test]
    fn identity_round_trips_collection_and_strings() {
        let identity = IdentityPayload {
            vendor_id: 0x28DE,
            product_id: 0x1304,
            version_number: 2,
            manufacturer: "Valve Software".to_string(),
            product: "Steam Controller Puck".to_string(),
            serial: "FXB9961303C9C".to_string(),
            collections: vec![CollectionDescriptor {
                role: CollectionRole::PuckMain,
                interface_number: 2,
                usage_page: 0x0001,
                usage: 0x0002,
                input_report_len: 54,
                output_report_len: 64,
                feature_report_len: 64,
            }],
        };

        assert_eq!(
            IdentityPayload::decode(&identity.to_bytes().unwrap()).unwrap(),
            identity
        );
    }

    #[test]
    fn write_report_preserves_prefix_bytes() {
        let report = WriteReport {
            interface_number: 2,
            timeout_ms: 250,
            data: vec![0x00, 0x80, 0xAA, 0xBB],
        };

        assert_eq!(
            WriteReport::decode(&report.to_bytes().unwrap()).unwrap(),
            report
        );
    }

    #[test]
    fn short_input_report_round_trips() {
        let report = InputReport {
            interface_number: 2,
            role: CollectionRole::PuckMain,
            host_monotonic_us: 123,
            data: vec![0x79, 0x02],
        };

        assert_eq!(
            InputReport::decode(&report.to_bytes().unwrap()).unwrap(),
            report
        );
    }
}
