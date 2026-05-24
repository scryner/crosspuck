use crate::hid_backend::{HostHidBackend, HostHidError};
use crosspuck_core::hid::{open_path_with_new_api, snapshot_for_filter, HidFilter};
use crosspuck_core::protocol::{
    Frame, GetFeature, Hello, HelloOk, IdentityPayload, InputAttach, InputAttachOk, InputReport,
    MessageType, ProtocolError, SetFeature, SetOutput, StatusCode, WireDecode, WirePayload,
    WriteReport, CONTROL_PAYLOAD_LIMIT, INPUT_PAYLOAD_LIMIT, PROTOCOL_VERSION,
};
use crosspuck_core::transport::{ChannelStream, TransportError, TransportListeners};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostRuntimeState {
    Starting,
    Listening,
    PuckDisconnected,
    PuckConnected { serial: String },
    GuestConnected { session_id: u32, serial: String },
    Degraded { reason: String },
}

impl HostRuntimeState {
    pub fn menu_label(&self) -> String {
        match self {
            Self::Starting => "시작 중".to_string(),
            Self::Listening => "Guest 대기 중".to_string(),
            Self::PuckDisconnected => "Puck 연결 안됨".to_string(),
            Self::PuckConnected { serial } => format!("Puck 연결됨 ({serial})"),
            Self::GuestConnected { session_id, serial } => {
                format!("Guest proxy 연결됨 ({serial}, session={session_id})")
            }
            Self::Degraded { reason } => format!("오류 ({reason})"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    state: Arc<Mutex<HostRuntimeState>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HostRuntimeState::Starting)),
        }
    }

    pub fn snapshot(&self) -> HostRuntimeState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| HostRuntimeState::Degraded {
                reason: "state lock poisoned".to_string(),
            })
    }

    fn set(&self, state: HostRuntimeState) {
        if let Ok(mut current) = self.state.lock() {
            *current = state;
        }
    }
}

pub struct HostServiceHandle {
    _thread: JoinHandle<()>,
}

pub fn start_host_service(app_state: AppState) -> HostServiceHandle {
    let thread = thread::spawn(move || run_supervisor(app_state));
    HostServiceHandle { _thread: thread }
}

fn run_supervisor(app_state: AppState) {
    app_state.set(HostRuntimeState::Starting);

    let listeners = match TransportListeners::bind_default() {
        Ok(listeners) => listeners,
        Err(error) => {
            app_state.set(HostRuntimeState::Degraded {
                reason: format!("listen failed: {error}"),
            });
            return;
        }
    };
    app_state.set(HostRuntimeState::Listening);

    let mut next_session_id = 1_u32;
    loop {
        let mut control = match listeners.accept_control() {
            Ok(control) => control,
            Err(error) => {
                app_state.set(HostRuntimeState::Degraded {
                    reason: format!("control accept failed: {error}"),
                });
                continue;
            }
        };

        let session_id = next_session_id;
        next_session_id = next_session_id.wrapping_add(1).max(1);
        match handle_session(&listeners, &mut control, session_id, &app_state) {
            Ok(()) => {}
            Err(RuntimeError::DeviceUnavailable(_)) => {
                app_state.set(HostRuntimeState::PuckDisconnected);
            }
            Err(error) => {
                app_state.set(HostRuntimeState::Degraded {
                    reason: format!("session failed: {error}"),
                });
            }
        }
    }
}

fn handle_session(
    listeners: &TransportListeners,
    control: &mut ChannelStream,
    session_id: u32,
    app_state: &AppState,
) -> Result<(), RuntimeError> {
    let hello_frame = control.read_frame()?;
    if hello_frame.header.message_type != MessageType::Hello {
        return Err(RuntimeError::UnexpectedMessage {
            expected: MessageType::Hello,
            actual: hello_frame.header.message_type,
        });
    }
    let hello = Hello::decode(&hello_frame.payload)?;
    if hello.guest_protocol_version != PROTOCOL_VERSION as u16 {
        let hello_ok = hello_ok_with_status(StatusCode::ProtocolError, session_id, 0);
        write_payload(control, hello_frame.header.id, &hello_ok)?;
        return Err(RuntimeError::ProtocolVersionMismatch(
            hello.guest_protocol_version,
        ));
    }

    let snapshot = match snapshot_for_filter(&HidFilter::steam_puck()) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            app_state.set(HostRuntimeState::PuckDisconnected);
            let hello_ok = hello_ok_with_status(StatusCode::DeviceDisconnected, session_id, 0);
            write_payload(control, hello_frame.header.id, &hello_ok)?;
            return Err(RuntimeError::DeviceUnavailable(error.to_string()));
        }
    };
    let identity = IdentityPayload::try_from(&snapshot)?;
    let backend = HostHidBackend::new(snapshot);
    app_state.set(HostRuntimeState::PuckConnected {
        serial: identity.serial.clone(),
    });

    write_payload(
        control,
        hello_frame.header.id,
        &HelloOk::success(session_id, identity.default_input_report_len()),
    )?;
    write_payload(control, 0, &identity)?;

    let mut input = listeners.accept_input()?;
    input.set_write_timeout(Some(Duration::from_millis(250)))?;
    let attach_frame = input.read_frame()?;
    if attach_frame.header.message_type != MessageType::InputAttach {
        return Err(RuntimeError::UnexpectedMessage {
            expected: MessageType::InputAttach,
            actual: attach_frame.header.message_type,
        });
    }
    let attach = InputAttach::decode(&attach_frame.payload)?;
    let attach_status = if attach.session_id == session_id {
        StatusCode::Ok
    } else {
        StatusCode::BadRequest
    };
    write_payload(
        &mut input,
        attach_frame.header.id,
        &InputAttachOk {
            status: attach_status,
            input_report_len: identity.default_input_report_len(),
            first_input_seq: 1,
        },
    )?;
    if !attach_status.is_ok() {
        return Err(RuntimeError::SessionMismatch {
            expected: session_id,
            actual: attach.session_id,
        });
    }

    app_state.set(HostRuntimeState::GuestConnected {
        session_id,
        serial: identity.serial.clone(),
    });

    let input_running = Arc::new(AtomicBool::new(true));
    let input_thread = spawn_input_stream(input, backend.clone(), Arc::clone(&input_running))?;
    let control_result = run_control_loop(control, &backend);
    input_running.store(false, Ordering::Relaxed);
    let _ = input_thread.join();
    backend.cleanup_feedback();
    app_state.set(HostRuntimeState::PuckConnected {
        serial: identity.serial,
    });
    control_result
}

fn run_control_loop(
    control: &mut ChannelStream,
    backend: &HostHidBackend,
) -> Result<(), RuntimeError> {
    loop {
        let frame = match control.read_frame() {
            Ok(frame) => frame,
            Err(TransportError::Io(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof
                    || error.kind() == std::io::ErrorKind::ConnectionReset =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };

        match frame.header.message_type {
            MessageType::GetFeature => {
                let request = GetFeature::decode(&frame.payload)?;
                write_payload(control, frame.header.id, &backend.get_feature(&request))?;
            }
            MessageType::SetFeature => {
                let request = SetFeature::decode(&frame.payload)?;
                write_payload(control, frame.header.id, &backend.set_feature(&request))?;
            }
            MessageType::SetOutput => {
                let request = SetOutput::decode(&frame.payload)?;
                write_payload(control, frame.header.id, &backend.set_output(&request))?;
            }
            MessageType::Write => {
                let request = WriteReport::decode(&frame.payload)?;
                write_payload(control, frame.header.id, &backend.write_report(&request))?;
            }
            actual => {
                return Err(RuntimeError::UnexpectedControlMessage(actual));
            }
        }
    }
}

fn spawn_input_stream(
    mut input: ChannelStream,
    backend: HostHidBackend,
    running: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, RuntimeError> {
    let collection = backend.input_collection()?;
    let interface_number = u8::try_from(collection.interface_number)
        .map_err(|_| RuntimeError::InvalidInterfaceNumber(collection.interface_number))?;
    let role = collection.role.into();

    Ok(thread::spawn(move || {
        let Ok(device) = open_path_with_new_api(&collection.path) else {
            return;
        };
        let read_len = usize::from(collection.input_report_len).max(64);
        let mut buffer = vec![0_u8; read_len];
        let start = Instant::now();
        let mut sequence = 1_u32;

        while running.load(Ordering::Relaxed) {
            match device.read_timeout(&mut buffer, 10) {
                Ok(0) => {}
                Ok(read) => {
                    let report = InputReport {
                        interface_number,
                        role,
                        host_monotonic_us: start.elapsed().as_micros().min(u128::from(u64::MAX))
                            as u64,
                        data: buffer[..read].to_vec(),
                    };
                    if write_payload(&mut input, sequence, &report).is_err() {
                        break;
                    }
                    sequence = sequence.wrapping_add(1).max(1);
                }
                Err(_) => break,
            }
        }
    }))
}

fn hello_ok_with_status(
    status: StatusCode,
    session_id: u32,
    default_input_report_len: u16,
) -> HelloOk {
    HelloOk {
        status,
        protocol_version: PROTOCOL_VERSION as u16,
        session_id,
        control_payload_limit: CONTROL_PAYLOAD_LIMIT as u16,
        input_payload_limit: INPUT_PAYLOAD_LIMIT as u16,
        default_input_report_len,
    }
}

fn write_payload<T: WirePayload>(
    stream: &mut ChannelStream,
    id: u32,
    payload: &T,
) -> Result<(), RuntimeError> {
    stream.write_frame(&Frame::new(T::MESSAGE_TYPE, id, payload.to_bytes()?))?;
    Ok(())
}

#[derive(Debug)]
enum RuntimeError {
    Transport(TransportError),
    Protocol(ProtocolError),
    HostHid(HostHidError),
    UnexpectedMessage {
        expected: MessageType,
        actual: MessageType,
    },
    UnexpectedControlMessage(MessageType),
    ProtocolVersionMismatch(u16),
    DeviceUnavailable(String),
    SessionMismatch {
        expected: u32,
        actual: u32,
    },
    InvalidInterfaceNumber(i32),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::Protocol(error) => write!(f, "{error}"),
            Self::HostHid(error) => write!(f, "{error}"),
            Self::UnexpectedMessage { expected, actual } => {
                write!(
                    f,
                    "unexpected message: expected {expected:?}, got {actual:?}"
                )
            }
            Self::UnexpectedControlMessage(message) => {
                write!(f, "unexpected control message: {message:?}")
            }
            Self::ProtocolVersionMismatch(version) => {
                write!(f, "unsupported guest protocol version: {version}")
            }
            Self::DeviceUnavailable(error) => write!(f, "device unavailable: {error}"),
            Self::SessionMismatch { expected, actual } => {
                write!(
                    f,
                    "input session mismatch: expected {expected}, got {actual}"
                )
            }
            Self::InvalidInterfaceNumber(interface_number) => {
                write!(f, "invalid HID interface number: {interface_number}")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<TransportError> for RuntimeError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

impl From<ProtocolError> for RuntimeError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

impl From<HostHidError> for RuntimeError {
    fn from(value: HostHidError) -> Self {
        Self::HostHid(value)
    }
}
