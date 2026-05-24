use crate::protocol::{
    Channel, FeatureResult, Frame, FrameIoError, GetFeature, Hello, HelloOk, IdentityPayload,
    InputAttach, InputAttachOk, InputQueueStats, InputReport, InputReportQueue, MessageType,
    ProtocolError, QueuedInputReport, SetFeature, SetFeatureResult, SetOutput, SetOutputResult,
    StatusCode, WireDecode, WirePayload, WriteReport, WriteResult, PROTOCOL_VERSION,
};
use crate::transport::{ChannelStream, TransportAddrs, TransportError};
use std::fmt;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct GuestTransportConfig {
    pub addrs: TransportAddrs,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub guest_pid: u32,
    pub guest_label: String,
}

impl Default for GuestTransportConfig {
    fn default() -> Self {
        Self {
            addrs: TransportAddrs::default(),
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(2),
            guest_pid: std::process::id(),
            guest_label: "crosspuck-guest".to_string(),
        }
    }
}

pub struct GuestTransportClient;

impl GuestTransportClient {
    pub fn connect(config: GuestTransportConfig) -> Result<GuestSession, GuestError> {
        let mut control = ChannelStream::connect_timeout(
            Channel::Control,
            config.addrs.control,
            config.connect_timeout,
        )?;
        control.set_read_timeout(Some(config.io_timeout))?;
        control.set_write_timeout(Some(config.io_timeout))?;

        let mut next_request_id = 1;
        let hello_id = next_request_id;
        next_request_id += 1;
        write_payload(&mut control, hello_id, &Hello::new(config.guest_pid))?;

        let hello_ok_frame = control.read_frame()?;
        let hello_ok = decode_expected::<HelloOk>(&hello_ok_frame)?;
        if hello_ok.protocol_version != PROTOCOL_VERSION as u16 {
            return Err(GuestError::ProtocolVersionMismatch {
                expected: PROTOCOL_VERSION as u16,
                actual: hello_ok.protocol_version,
            });
        }
        if hello_ok_frame.header.id != hello_id {
            return Err(GuestError::UnexpectedResponseId {
                expected: hello_id,
                actual: hello_ok_frame.header.id,
            });
        }
        if !hello_ok.status.is_ok() {
            return Err(GuestError::NonOkStatus(hello_ok.status));
        }

        let identity_frame = control.read_frame()?;
        let identity = decode_expected::<IdentityPayload>(&identity_frame)?;
        let session_id = hello_ok.session_id;

        let mut input = ChannelStream::connect_timeout(
            Channel::Input,
            config.addrs.input,
            config.connect_timeout,
        )?;
        input.set_read_timeout(Some(config.io_timeout))?;
        input.set_write_timeout(Some(config.io_timeout))?;

        let attach_id = next_request_id;
        next_request_id += 1;
        write_payload(
            &mut input,
            attach_id,
            &InputAttach {
                session_id,
                last_seen_input_seq: 0,
            },
        )?;

        let attach_ok_frame = input.read_frame()?;
        let attach_ok = decode_expected::<InputAttachOk>(&attach_ok_frame)?;
        if attach_ok_frame.header.id != attach_id {
            return Err(GuestError::UnexpectedResponseId {
                expected: attach_id,
                actual: attach_ok_frame.header.id,
            });
        }
        if !attach_ok.status.is_ok() {
            return Err(GuestError::NonOkStatus(attach_ok.status));
        }

        Ok(GuestSession {
            info: GuestSessionInfo {
                identity,
                session_id,
                guest_label: config.guest_label,
            },
            control: GuestControl {
                control,
                next_request_id,
            },
            input: GuestInput {
                input,
                input_queue: InputReportQueue::default(),
            },
        })
    }
}

#[derive(Clone, Debug)]
pub struct GuestSessionInfo {
    pub identity: IdentityPayload,
    pub session_id: u32,
    pub guest_label: String,
}

pub struct GuestSession {
    info: GuestSessionInfo,
    control: GuestControl,
    input: GuestInput,
}

impl GuestSession {
    pub fn identity(&self) -> &IdentityPayload {
        &self.info.identity
    }

    pub fn session_id(&self) -> u32 {
        self.info.session_id
    }

    pub fn guest_label(&self) -> &str {
        &self.info.guest_label
    }

    pub fn input_queue_len(&self) -> usize {
        self.input.input_queue_len()
    }

    pub fn input_queue_stats(&self) -> InputQueueStats {
        self.input.input_queue_stats()
    }

    pub fn read_input_report(&mut self) -> Result<QueuedInputReport, GuestError> {
        self.input.read_input_report()
    }

    pub fn set_input_read_timeout(&self, timeout: Option<Duration>) -> Result<(), GuestError> {
        self.input.set_read_timeout(timeout)
    }

    pub fn get_feature(
        &mut self,
        interface_number: u8,
        report_id: u8,
        requested_len: u16,
        timeout_ms: u16,
    ) -> Result<FeatureResult, GuestError> {
        self.control
            .get_feature(interface_number, report_id, requested_len, timeout_ms)
    }

    pub fn set_feature(
        &mut self,
        interface_number: u8,
        timeout_ms: u16,
        payload: &[u8],
    ) -> Result<SetFeatureResult, GuestError> {
        self.control
            .set_feature(interface_number, timeout_ms, payload)
    }

    pub fn set_output(
        &mut self,
        interface_number: u8,
        timeout_ms: u16,
        payload: &[u8],
    ) -> Result<SetOutputResult, GuestError> {
        self.control
            .set_output(interface_number, timeout_ms, payload)
    }

    pub fn write_report(
        &mut self,
        interface_number: u8,
        timeout_ms: u16,
        payload: &[u8],
    ) -> Result<WriteResult, GuestError> {
        self.control
            .write_report(interface_number, timeout_ms, payload)
    }

    pub fn into_parts(self) -> GuestSessionParts {
        GuestSessionParts {
            info: self.info,
            control: self.control,
            input: self.input,
        }
    }
}

pub struct GuestSessionParts {
    pub info: GuestSessionInfo,
    pub control: GuestControl,
    pub input: GuestInput,
}

pub struct GuestControl {
    control: ChannelStream,
    next_request_id: u32,
}

impl GuestControl {
    pub fn get_feature(
        &mut self,
        interface_number: u8,
        report_id: u8,
        requested_len: u16,
        timeout_ms: u16,
    ) -> Result<FeatureResult, GuestError> {
        let request = GetFeature {
            interface_number,
            report_id,
            requested_len,
            timeout_ms,
        };
        self.control_request::<_, FeatureResult>(&request)
    }

    pub fn set_feature(
        &mut self,
        interface_number: u8,
        timeout_ms: u16,
        payload: &[u8],
    ) -> Result<SetFeatureResult, GuestError> {
        let request = SetFeature {
            interface_number,
            timeout_ms,
            data: payload.to_vec(),
        };
        self.control_request::<_, SetFeatureResult>(&request)
    }

    pub fn set_output(
        &mut self,
        interface_number: u8,
        timeout_ms: u16,
        payload: &[u8],
    ) -> Result<SetOutputResult, GuestError> {
        let request = SetOutput {
            interface_number,
            timeout_ms,
            data: payload.to_vec(),
        };
        self.control_request::<_, SetOutputResult>(&request)
    }

    pub fn write_report(
        &mut self,
        interface_number: u8,
        timeout_ms: u16,
        payload: &[u8],
    ) -> Result<WriteResult, GuestError> {
        let request = WriteReport {
            interface_number,
            timeout_ms,
            data: payload.to_vec(),
        };
        self.control_request::<_, WriteResult>(&request)
    }

    fn control_request<T, R>(&mut self, request: &T) -> Result<R, GuestError>
    where
        T: WirePayload,
        R: WirePayload,
    {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);

        write_payload(&mut self.control, request_id, request)?;
        let response = self.control.read_frame()?;
        if response.header.id != request_id {
            return Err(GuestError::UnexpectedResponseId {
                expected: request_id,
                actual: response.header.id,
            });
        }
        decode_expected::<R>(&response)
    }
}

pub struct GuestInput {
    input: ChannelStream,
    input_queue: InputReportQueue,
}

impl GuestInput {
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<(), GuestError> {
        self.input.set_read_timeout(timeout)?;
        Ok(())
    }

    pub fn input_queue_len(&self) -> usize {
        self.input_queue.len()
    }

    pub fn input_queue_stats(&self) -> InputQueueStats {
        self.input_queue.stats()
    }

    pub fn read_input_report(&mut self) -> Result<QueuedInputReport, GuestError> {
        if let Some(report) = self.input_queue.pop() {
            return Ok(report);
        }

        let frame = self.input.read_frame()?;
        match frame.header.message_type {
            MessageType::InputReport => {
                let report = InputReport::decode(&frame.payload)?;
                let queued = QueuedInputReport::from_wire(frame.header.id, report);
                self.input_queue.push(queued);
                self.input_queue
                    .pop()
                    .ok_or(GuestError::InputQueueUnexpectedlyEmpty)
            }
            actual => Err(GuestError::UnexpectedMessage {
                expected: MessageType::InputReport,
                actual,
            }),
        }
    }
}

fn write_payload<T: WirePayload>(
    stream: &mut ChannelStream,
    id: u32,
    payload: &T,
) -> Result<(), GuestError> {
    let frame = Frame::new(T::MESSAGE_TYPE, id, payload.to_bytes()?);
    stream.write_frame(&frame)?;
    Ok(())
}

fn decode_expected<T: WirePayload>(frame: &Frame) -> Result<T, GuestError> {
    if frame.header.message_type != T::MESSAGE_TYPE {
        return Err(GuestError::UnexpectedMessage {
            expected: T::MESSAGE_TYPE,
            actual: frame.header.message_type,
        });
    }
    T::decode(&frame.payload).map_err(Into::into)
}

#[derive(Debug)]
pub enum GuestError {
    Transport(TransportError),
    Protocol(ProtocolError),
    UnexpectedMessage {
        expected: MessageType,
        actual: MessageType,
    },
    UnexpectedResponseId {
        expected: u32,
        actual: u32,
    },
    ProtocolVersionMismatch {
        expected: u16,
        actual: u16,
    },
    NonOkStatus(StatusCode),
    InputQueueUnexpectedlyEmpty,
}

impl fmt::Display for GuestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::Protocol(error) => write!(f, "{error}"),
            Self::UnexpectedMessage { expected, actual } => {
                write!(
                    f,
                    "unexpected message: expected {expected:?}, got {actual:?}"
                )
            }
            Self::UnexpectedResponseId { expected, actual } => {
                write!(
                    f,
                    "unexpected response id: expected {expected}, got {actual}"
                )
            }
            Self::ProtocolVersionMismatch { expected, actual } => {
                write!(
                    f,
                    "protocol version mismatch: expected {expected}, got {actual}"
                )
            }
            Self::NonOkStatus(status) => write!(f, "non-ok status: {status}"),
            Self::InputQueueUnexpectedlyEmpty => f.write_str("input queue unexpectedly empty"),
        }
    }
}

impl GuestError {
    pub fn is_timeout_or_would_block(&self) -> bool {
        matches!(
            self,
            Self::Transport(TransportError::Io(io_error))
                if io_error.kind() == std::io::ErrorKind::WouldBlock
                    || io_error.kind() == std::io::ErrorKind::TimedOut
        ) || matches!(
            self,
            Self::Transport(TransportError::Frame(FrameIoError::Io(io_error)))
                if io_error.kind() == std::io::ErrorKind::WouldBlock
                    || io_error.kind() == std::io::ErrorKind::TimedOut
        )
    }
}

impl std::error::Error for GuestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

impl From<TransportError> for GuestError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

impl From<ProtocolError> for GuestError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        CollectionDescriptor, CollectionRole, FeatureResult, InputReport, SetFeature,
        SetFeatureResult, SetOutput, SetOutputResult, StatusCode, WriteReport, WriteResult,
    };
    use crate::transport::{TransportAddrs, TransportListeners};
    use std::sync::mpsc;
    use std::thread;

    fn identity() -> IdentityPayload {
        IdentityPayload {
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
        }
    }

    #[test]
    fn guest_client_uses_shared_runtime_for_handshake_and_requests() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);

            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xAABB_CCDD, 54),
            )
            .unwrap();
            write_payload(&mut control, 0, &identity()).unwrap();

            let mut input = listeners.accept_input().unwrap();
            let attach = input.read_frame().unwrap();
            assert_eq!(attach.header.message_type, MessageType::InputAttach);
            write_payload(
                &mut input,
                attach.header.id,
                &InputAttachOk {
                    status: StatusCode::Ok,
                    input_report_len: 54,
                    first_input_seq: 1,
                },
            )
            .unwrap();
            write_payload(
                &mut input,
                1,
                &InputReport {
                    interface_number: 2,
                    role: CollectionRole::PuckMain,
                    host_monotonic_us: 42,
                    data: vec![0x79, 0x02],
                },
            )
            .unwrap();

            let feature = control.read_frame().unwrap();
            assert_eq!(feature.header.message_type, MessageType::GetFeature);
            write_payload(
                &mut control,
                feature.header.id,
                &FeatureResult {
                    status: StatusCode::Ok,
                    os_error: 0,
                    data: vec![0x02, 0xB4],
                },
            )
            .unwrap();

            let write = control.read_frame().unwrap();
            assert_eq!(write.header.message_type, MessageType::Write);
            let write_request = WriteReport::decode(&write.payload).unwrap();
            assert_eq!(write_request.data, vec![0x00, 0x80, 0xAA, 0xBB]);
            write_payload(
                &mut control,
                write.header.id,
                &WriteResult {
                    status: StatusCode::Ok,
                    bytes_written: write_request.data.len() as u16,
                    os_error: 0,
                },
            )
            .unwrap();

            let set_feature = control.read_frame().unwrap();
            assert_eq!(set_feature.header.message_type, MessageType::SetFeature);
            let set_feature_request = SetFeature::decode(&set_feature.payload).unwrap();
            assert_eq!(set_feature_request.data, vec![0x01, 0x83, 0x01, 0x00]);
            write_payload(
                &mut control,
                set_feature.header.id,
                &SetFeatureResult {
                    status: StatusCode::Ok,
                    bytes_accepted: set_feature_request.data.len() as u16,
                    os_error: 0,
                },
            )
            .unwrap();

            let set_output = control.read_frame().unwrap();
            assert_eq!(set_output.header.message_type, MessageType::SetOutput);
            let set_output_request = SetOutput::decode(&set_output.payload).unwrap();
            assert_eq!(
                set_output_request.data,
                vec![0x80, 0x00, 0x00, 0x00, 0x34, 0x12, 0x00, 0x78, 0x56, 0x00]
            );
            write_payload(
                &mut control,
                set_output.header.id,
                &SetOutputResult {
                    status: StatusCode::Ok,
                    bytes_accepted: set_output_request.data.len() as u16,
                    os_error: 0,
                },
            )
            .unwrap();
        });

        ready_rx.recv().unwrap();
        let mut session = GuestTransportClient::connect(GuestTransportConfig {
            addrs,
            guest_pid: 1234,
            guest_label: "test".to_string(),
            ..GuestTransportConfig::default()
        })
        .unwrap();

        assert_eq!(session.identity().serial, "FXB9961303C9C");
        let report = session.read_input_report().unwrap();
        assert_eq!(report.sequence, 1);
        assert_eq!(report.data, vec![0x79, 0x02]);

        let result = session.get_feature(2, 0x02, 64, 100).unwrap();
        assert_eq!(result.status, StatusCode::Ok);
        assert_eq!(result.data, vec![0x02, 0xB4]);

        let write = session
            .write_report(2, 100, &[0x00, 0x80, 0xAA, 0xBB])
            .unwrap();
        assert_eq!(write.status, StatusCode::Ok);
        assert_eq!(write.bytes_written, 4);

        let set_feature = session
            .set_feature(2, 100, &[0x01, 0x83, 0x01, 0x00])
            .unwrap();
        assert_eq!(set_feature.status, StatusCode::Ok);
        assert_eq!(set_feature.bytes_accepted, 4);

        let set_output = session
            .set_output(
                2,
                100,
                &[0x80, 0x00, 0x00, 0x00, 0x34, 0x12, 0x00, 0x78, 0x56, 0x00],
            )
            .unwrap();
        assert_eq!(set_output.status, StatusCode::Ok);
        assert_eq!(set_output.bytes_accepted, 10);

        server.join().unwrap();
    }

    #[test]
    fn guest_client_can_reconnect_with_same_runtime() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            for session_id in [1_u32, 2_u32] {
                let mut control = listeners.accept_control().unwrap();
                let hello = control.read_frame().unwrap();
                write_payload(
                    &mut control,
                    hello.header.id,
                    &HelloOk::success(session_id, 54),
                )
                .unwrap();
                write_payload(&mut control, 0, &identity()).unwrap();

                let mut input = listeners.accept_input().unwrap();
                let attach = input.read_frame().unwrap();
                write_payload(
                    &mut input,
                    attach.header.id,
                    &InputAttachOk {
                        status: StatusCode::Ok,
                        input_report_len: 54,
                        first_input_seq: 1,
                    },
                )
                .unwrap();
            }
        });

        ready_rx.recv().unwrap();
        for _ in 0..2 {
            let session = GuestTransportClient::connect(GuestTransportConfig {
                addrs,
                guest_pid: 1234,
                guest_label: "reconnect-test".to_string(),
                ..GuestTransportConfig::default()
            })
            .unwrap();
            assert_eq!(session.identity().serial, "FXB9961303C9C");
        }

        server.join().unwrap();
    }
}
