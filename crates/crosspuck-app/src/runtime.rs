use crate::hid_backend::{
    set_host_log_session_trace_id, HostBackend, HostHidError, RealHostBackend,
};
use crosspuck_core::hid::{snapshot_for_filter, HidFilter};
use crosspuck_core::protocol::{
    session_trace_label, Frame, FrameIoError, GetFeature, Hello, HelloOk, IdentityPayload,
    InputAttach, InputAttachOk, InputReport, LogSeverity, MessageType, ProtocolError, SetFeature,
    SetOutput, StatusCode, WireDecode, WirePayload, WriteReport, CONTROL_PAYLOAD_LIMIT,
    INPUT_PAYLOAD_LIMIT, PROTOCOL_VERSION,
};
use crosspuck_core::transport::{
    ChannelStream, TransportAddrs, TransportError, TransportListeners,
};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const INPUT_ATTACH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostRuntimeState {
    Starting,
    Listening {
        control_addr: SocketAddr,
        input_addr: SocketAddr,
    },
    PuckDisconnected,
    PuckConnected {
        serial: String,
    },
    GuestConnected {
        session_id: u32,
        session_trace_id: u32,
        serial: String,
        guest_pid: u32,
    },
    Degraded {
        reason: String,
    },
    Stopping,
}

impl HostRuntimeState {
    pub fn menu_view(&self) -> MenuView {
        match self {
            Self::Starting => MenuView::new("Starting", "-", "-", "-"),
            Self::Listening {
                control_addr,
                input_addr,
            } => MenuView::new(
                "Waiting for guest",
                "-",
                &format!("{control_addr}, {input_addr}"),
                "-",
            ),
            Self::PuckDisconnected => MenuView::new("Puck disconnected", "-", "-", "-"),
            Self::PuckConnected { serial } => {
                MenuView::new("Puck connected", serial.as_str(), "-", "-")
            }
            Self::GuestConnected {
                session_id,
                session_trace_id,
                serial,
                guest_pid,
            } => MenuView::new(
                "Guest proxy connected",
                serial.as_str(),
                &format!(
                    "pid={guest_pid}, session={}, id={session_id}",
                    session_trace_label(*session_trace_id)
                ),
                "-",
            ),
            Self::Degraded { reason } => MenuView::new("Error", "-", "-", reason.as_str()),
            Self::Stopping => MenuView::new("Stopping", "-", "-", "-"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuView {
    pub status: String,
    pub puck: String,
    pub guest: String,
    pub error: String,
}

impl MenuView {
    fn new(status: &str, puck: &str, guest: &str, error: &str) -> Self {
        Self {
            status: status.to_string(),
            puck: puck.to_string(),
            guest: guest.to_string(),
            error: error.to_string(),
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
    stop: Arc<AtomicBool>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostServiceConfig {
    pub guest_log_level_override: Option<LogSeverity>,
}

impl HostServiceHandle {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut thread) = self.thread.lock() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Clone for HostServiceHandle {
    fn clone(&self) -> Self {
        Self {
            stop: Arc::clone(&self.stop),
            thread: Arc::clone(&self.thread),
        }
    }
}

pub fn start_host_service_with_config(
    app_state: AppState,
    config: HostServiceConfig,
) -> HostServiceHandle {
    start_host_service_on_with_config(app_state, TransportAddrs::default(), config)
}

#[cfg(test)]
fn start_host_service_on(app_state: AppState, addrs: TransportAddrs) -> HostServiceHandle {
    start_host_service_on_with_config(app_state, addrs, HostServiceConfig::default())
}

fn start_host_service_on_with_config(
    app_state: AppState,
    addrs: TransportAddrs,
    config: HostServiceConfig,
) -> HostServiceHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = thread::spawn(move || run_supervisor(app_state, thread_stop, addrs, config));
    HostServiceHandle {
        stop,
        thread: Arc::new(Mutex::new(Some(thread))),
    }
}

fn run_supervisor(
    app_state: AppState,
    stop: Arc<AtomicBool>,
    addrs: TransportAddrs,
    config: HostServiceConfig,
) {
    app_state.set(HostRuntimeState::Starting);

    while !stop.load(Ordering::Relaxed) {
        let listeners = match TransportListeners::bind(addrs) {
            Ok(listeners) => listeners,
            Err(error) => {
                app_state.set(HostRuntimeState::Degraded {
                    reason: format!("listen failed: {error}"),
                });
                sleep_until_stop(&stop, Duration::from_secs(1));
                continue;
            }
        };

        if let Err(error) = listeners.set_nonblocking(true) {
            app_state.set(HostRuntimeState::Degraded {
                reason: format!("listen setup failed: {error}"),
            });
            sleep_until_stop(&stop, Duration::from_secs(1));
            continue;
        }

        let addrs = match listeners.local_addrs() {
            Ok(addrs) => addrs,
            Err(error) => {
                app_state.set(HostRuntimeState::Degraded {
                    reason: format!("listen address failed: {error}"),
                });
                sleep_until_stop(&stop, Duration::from_secs(1));
                continue;
            }
        };
        app_state.set(HostRuntimeState::Listening {
            control_addr: addrs.control,
            input_addr: addrs.input,
        });

        run_accept_loop(&listeners, &app_state, &stop, config);
    }

    app_state.set(HostRuntimeState::Stopping);
}

fn run_accept_loop(
    listeners: &TransportListeners,
    app_state: &AppState,
    stop: &Arc<AtomicBool>,
    config: HostServiceConfig,
) {
    let mut next_session_id = 1_u32;
    let mut next_puck_probe = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        if Instant::now() >= next_puck_probe {
            refresh_idle_puck_state(listeners, app_state);
            next_puck_probe = Instant::now() + Duration::from_secs(1);
        }

        let mut control = match listeners.accept_control() {
            Ok(control) => control,
            Err(error) if is_would_block(&error) => {
                sleep_until_stop(stop, Duration::from_millis(50));
                continue;
            }
            Err(error) => {
                app_state.set(HostRuntimeState::Degraded {
                    reason: format!("control accept failed: {error}"),
                });
                sleep_until_stop(stop, Duration::from_millis(250));
                continue;
            }
        };
        let _ = control.set_read_timeout(Some(Duration::from_millis(250)));
        let _ = control.set_write_timeout(Some(Duration::from_millis(250)));

        let session_id = next_session_id;
        let session_trace_id = new_session_trace_id();
        next_session_id = next_session_id.wrapping_add(1).max(1);
        match handle_session(
            listeners,
            &mut control,
            session_id,
            session_trace_id,
            config.guest_log_level_override,
            app_state,
            stop,
        ) {
            Ok(()) => {}
            Err(RuntimeError::DeviceUnavailable(_)) => {
                app_state.set(HostRuntimeState::PuckDisconnected);
            }
            Err(error) if stop.load(Ordering::Relaxed) && error.is_timeout() => {}
            Err(error) => {
                app_state.set(HostRuntimeState::Degraded {
                    reason: format!("session failed: {error}"),
                });
            }
        }
    }
}

fn refresh_idle_puck_state(listeners: &TransportListeners, app_state: &AppState) {
    if listeners.local_addrs().is_err() {
        return;
    }
    match snapshot_for_filter(&HidFilter::steam_puck()) {
        Ok(snapshot) => app_state.set(HostRuntimeState::PuckConnected {
            serial: snapshot.identity.serial,
        }),
        Err(_) => app_state.set(HostRuntimeState::PuckDisconnected),
    }
}

fn handle_session(
    listeners: &TransportListeners,
    control: &mut ChannelStream,
    session_id: u32,
    session_trace_id: u32,
    guest_log_level_override: Option<LogSeverity>,
    app_state: &AppState,
    stop: &Arc<AtomicBool>,
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
        let hello_ok =
            hello_ok_with_status(StatusCode::ProtocolError, session_id, session_trace_id, 0);
        write_payload(control, hello_frame.header.id, &hello_ok)?;
        return Err(RuntimeError::ProtocolVersionMismatch(
            hello.guest_protocol_version,
        ));
    }

    let snapshot = match snapshot_for_filter(&HidFilter::steam_puck()) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            app_state.set(HostRuntimeState::PuckDisconnected);
            let hello_ok = hello_ok_with_status(
                StatusCode::DeviceDisconnected,
                session_id,
                session_trace_id,
                0,
            );
            write_payload(control, hello_frame.header.id, &hello_ok)?;
            return Err(RuntimeError::DeviceUnavailable(error.to_string()));
        }
    };
    let identity = IdentityPayload::try_from(&snapshot)?;
    let backend = Arc::new(RealHostBackend::new(snapshot, identity)?);
    handle_session_with_backend(
        listeners,
        control,
        SessionStart {
            session_id,
            session_trace_id,
            guest_log_level_override,
            hello_request_id: hello_frame.header.id,
            guest_pid: hello.guest_pid,
        },
        app_state,
        backend,
        stop,
    )
}

#[derive(Clone, Copy, Debug)]
struct SessionStart {
    session_id: u32,
    session_trace_id: u32,
    guest_log_level_override: Option<LogSeverity>,
    hello_request_id: u32,
    guest_pid: u32,
}

fn handle_session_with_backend(
    listeners: &TransportListeners,
    control: &mut ChannelStream,
    session: SessionStart,
    app_state: &AppState,
    backend: Arc<dyn HostBackend>,
    stop: &Arc<AtomicBool>,
) -> Result<(), RuntimeError> {
    handle_session_with_backend_timeout(
        listeners,
        control,
        session,
        app_state,
        backend,
        stop,
        INPUT_ATTACH_TIMEOUT,
    )
}

fn handle_session_with_backend_timeout(
    listeners: &TransportListeners,
    control: &mut ChannelStream,
    session: SessionStart,
    app_state: &AppState,
    backend: Arc<dyn HostBackend>,
    stop: &Arc<AtomicBool>,
    input_attach_timeout: Duration,
) -> Result<(), RuntimeError> {
    let identity = backend.identity().clone();
    app_state.set(HostRuntimeState::PuckConnected {
        serial: identity.serial.clone(),
    });

    write_payload(
        control,
        session.hello_request_id,
        &HelloOk::success_with_trace_and_log_level(
            session.session_id,
            session.session_trace_id,
            identity.default_input_report_len(),
            session.guest_log_level_override,
        ),
    )?;
    if let Some(log_level) = session.guest_log_level_override {
        log::info!(
            "CrossPuck[{}] guest log level override={}",
            session_trace_label(session.session_trace_id),
            log_level.as_str()
        );
    }
    write_payload(control, 0, &identity)?;

    let mut input = accept_input_for_session(listeners, stop, input_attach_timeout)?;
    input.set_write_timeout(Some(Duration::from_millis(250)))?;
    input.set_read_timeout(Some(Duration::from_millis(250)))?;
    let attach_frame = input.read_frame()?;
    if attach_frame.header.message_type != MessageType::InputAttach {
        return Err(RuntimeError::UnexpectedMessage {
            expected: MessageType::InputAttach,
            actual: attach_frame.header.message_type,
        });
    }
    let attach = InputAttach::decode(&attach_frame.payload)?;
    let attach_status = if attach.session_id == session.session_id {
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
            expected: session.session_id,
            actual: attach.session_id,
        });
    }

    app_state.set(HostRuntimeState::GuestConnected {
        session_id: session.session_id,
        session_trace_id: session.session_trace_id,
        serial: identity.serial.clone(),
        guest_pid: session.guest_pid,
    });
    set_host_log_session_trace_id(Some(session.session_trace_id));

    let input_running = Arc::new(AtomicBool::new(true));
    let input_thread = spawn_input_stream(input, Arc::clone(&backend), Arc::clone(&input_running))?;
    let control_result =
        run_control_loop(control, backend.as_ref(), session.session_trace_id, stop);
    input_running.store(false, Ordering::Relaxed);
    let _ = input_thread.join();
    backend.cleanup_feedback();
    set_host_log_session_trace_id(None);
    app_state.set(HostRuntimeState::PuckConnected {
        serial: identity.serial,
    });
    control_result
}

fn run_control_loop(
    control: &mut ChannelStream,
    backend: &dyn HostBackend,
    session_trace_id: u32,
    stop: &Arc<AtomicBool>,
) -> Result<(), RuntimeError> {
    let session_trace = session_trace_label(session_trace_id);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let frame = match control.read_frame() {
            Ok(frame) => frame,
            Err(error) if is_disconnect(&error) => return Ok(()),
            Err(error) if is_timeout_or_would_block(&error) => continue,
            Err(error) => return Err(error.into()),
        };

        match frame.header.message_type {
            MessageType::GetFeature => {
                let request = GetFeature::decode(&frame.payload)?;
                let result = backend.get_feature(&request);
                if !result.status.is_ok() {
                    log::warn!(
                        "CrossPuck[{session_trace}] GET_FEATURE id={} interface={} report_id=0x{:02X} status={}",
                        frame.header.id, request.interface_number, request.report_id, result.status
                    );
                }
                write_payload(control, frame.header.id, &result)?;
            }
            MessageType::SetFeature => {
                let request = SetFeature::decode(&frame.payload)?;
                let result = backend.set_feature(&request);
                if !result.status.is_ok() {
                    log::warn!(
                        "CrossPuck[{session_trace}] SET_FEATURE id={} interface={} len={} status={}",
                        frame.header.id,
                        request.interface_number,
                        request.data.len(),
                        result.status
                    );
                }
                write_payload(control, frame.header.id, &result)?;
            }
            MessageType::SetOutput => {
                let request = SetOutput::decode(&frame.payload)?;
                let result = backend.set_output(&request);
                if !result.status.is_ok() {
                    log::warn!(
                        "CrossPuck[{session_trace}] SET_OUTPUT id={} interface={} len={} status={}",
                        frame.header.id,
                        request.interface_number,
                        request.data.len(),
                        result.status
                    );
                }
                write_payload(control, frame.header.id, &result)?;
            }
            MessageType::Write => {
                let request = WriteReport::decode(&frame.payload)?;
                let result = backend.write_report(&request);
                if !result.status.is_ok() {
                    log::warn!(
                        "CrossPuck[{session_trace}] WRITE id={} interface={} len={} status={}",
                        frame.header.id,
                        request.interface_number,
                        request.data.len(),
                        result.status
                    );
                }
                write_payload(control, frame.header.id, &result)?;
            }
            actual => {
                return Err(RuntimeError::UnexpectedControlMessage(actual));
            }
        }
    }
}

fn accept_input_for_session(
    listeners: &TransportListeners,
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<ChannelStream, RuntimeError> {
    let deadline = Instant::now() + timeout;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(RuntimeError::Stopping);
        }
        match listeners.accept_input() {
            Ok(input) => return Ok(input),
            Err(error) if is_would_block(&error) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(RuntimeError::InputAttachTimeout(timeout));
                }
                sleep_until_stop(stop, remaining.min(Duration::from_millis(50)));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn is_disconnect(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::Io(io_error)
            if io_error.kind() == std::io::ErrorKind::UnexpectedEof
                || io_error.kind() == std::io::ErrorKind::ConnectionReset
    ) || matches!(
        error,
        TransportError::Frame(FrameIoError::Io(io_error))
            if io_error.kind() == std::io::ErrorKind::UnexpectedEof
                || io_error.kind() == std::io::ErrorKind::ConnectionReset
    )
}

fn is_would_block(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::Io(io_error) if io_error.kind() == std::io::ErrorKind::WouldBlock
    )
}

fn is_timeout_or_would_block(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::Io(io_error)
            if io_error.kind() == std::io::ErrorKind::WouldBlock
                || io_error.kind() == std::io::ErrorKind::TimedOut
    ) || matches!(
        error,
        TransportError::Frame(FrameIoError::Io(io_error))
            if io_error.kind() == std::io::ErrorKind::WouldBlock
                || io_error.kind() == std::io::ErrorKind::TimedOut
    )
}

fn sleep_until_stop(stop: &Arc<AtomicBool>, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
}

fn new_session_trace_id() -> u32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(now.to_string().as_bytes());
    let digest = hasher.finalize();
    ((u32::from(digest[0])) << 12) | ((u32::from(digest[1])) << 4) | (u32::from(digest[2]) >> 4)
}

fn spawn_input_stream(
    mut input: ChannelStream,
    backend: Arc<dyn HostBackend>,
    running: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, RuntimeError> {
    Ok(thread::spawn(move || {
        let Ok(mut reader) = backend.open_input_reader() else {
            return;
        };
        let start = Instant::now();
        let mut sequence = 1_u32;

        while running.load(Ordering::Relaxed) {
            match reader.read_report(Duration::from_millis(10)) {
                Ok(None) => {}
                Ok(Some(input_report)) => {
                    let report = InputReport {
                        interface_number: input_report.descriptor.interface_number,
                        role: input_report.descriptor.role,
                        host_monotonic_us: start.elapsed().as_micros().min(u128::from(u64::MAX))
                            as u64,
                        data: input_report.data,
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
    session_trace_id: u32,
    default_input_report_len: u16,
) -> HelloOk {
    HelloOk {
        status,
        protocol_version: PROTOCOL_VERSION as u16,
        session_id,
        session_trace_id,
        guest_log_level_override: None,
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
    Stopping,
    InputAttachTimeout(Duration),
    SessionMismatch {
        expected: u32,
        actual: u32,
    },
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
            Self::Stopping => write!(f, "stopping"),
            Self::InputAttachTimeout(timeout) => {
                write!(f, "input attach timed out after {}ms", timeout.as_millis())
            }
            Self::SessionMismatch { expected, actual } => {
                write!(
                    f,
                    "input session mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl RuntimeError {
    fn is_timeout(&self) -> bool {
        matches!(self, Self::InputAttachTimeout(_))
            || matches!(self, Self::Transport(error) if is_timeout_or_would_block(error))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid_backend::{HostBackend, HostInputReport, InputDescriptor, InputReportReader};
    use crosspuck_core::guest::{GuestTransportClient, GuestTransportConfig};
    use crosspuck_core::protocol::{
        Channel, CollectionDescriptor, CollectionRole, FeatureResult, IdentityPayload,
        SetFeatureResult, SetOutputResult, StatusCode, WriteResult,
    };
    use crosspuck_core::transport::{ChannelStream, TransportAddrs, TransportListeners};
    use std::collections::VecDeque;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    #[derive(Clone)]
    struct FakeBackend {
        identity: IdentityPayload,
        reports: Arc<Mutex<VecDeque<Vec<u8>>>>,
        operations: Arc<Mutex<Vec<String>>>,
        cleanup_called: Arc<AtomicBool>,
    }

    impl FakeBackend {
        fn new(reports: Vec<Vec<u8>>) -> Self {
            Self {
                identity: test_identity(),
                reports: Arc::new(Mutex::new(reports.into())),
                operations: Arc::new(Mutex::new(Vec::new())),
                cleanup_called: Arc::new(AtomicBool::new(false)),
            }
        }

        fn operations(&self) -> Vec<String> {
            self.operations.lock().unwrap().clone()
        }

        fn cleanup_called(&self) -> bool {
            self.cleanup_called.load(Ordering::Relaxed)
        }

        fn push_operation(&self, operation: impl Into<String>) {
            self.operations.lock().unwrap().push(operation.into());
        }
    }

    impl HostBackend for FakeBackend {
        fn identity(&self) -> &IdentityPayload {
            &self.identity
        }

        fn open_input_reader(&self) -> Result<Box<dyn InputReportReader>, HostHidError> {
            Ok(Box::new(FakeInputReader {
                reports: Arc::clone(&self.reports),
            }))
        }

        fn get_feature(&self, request: &GetFeature) -> FeatureResult {
            self.push_operation(format!(
                "get_feature:{}:{:02X}:{}",
                request.interface_number, request.report_id, request.requested_len
            ));
            FeatureResult {
                status: StatusCode::Ok,
                os_error: 0,
                data: vec![request.report_id, 0xB4],
            }
        }

        fn set_feature(&self, request: &SetFeature) -> SetFeatureResult {
            self.push_operation(format!("set_feature:{:02X?}", request.data));
            SetFeatureResult {
                status: StatusCode::Ok,
                bytes_accepted: request.data.len() as u16,
                os_error: 0,
            }
        }

        fn set_output(&self, request: &SetOutput) -> SetOutputResult {
            self.push_operation(format!("set_output:{:02X?}", request.data));
            SetOutputResult {
                status: StatusCode::Ok,
                bytes_accepted: request.data.len() as u16,
                os_error: 0,
            }
        }

        fn write_report(&self, request: &WriteReport) -> WriteResult {
            self.push_operation(format!("write:{:02X?}", request.data));
            WriteResult {
                status: StatusCode::Ok,
                bytes_written: request.data.len() as u16,
                os_error: 0,
            }
        }

        fn cleanup_feedback(&self) {
            self.cleanup_called.store(true, Ordering::Relaxed);
        }
    }

    struct FakeInputReader {
        reports: Arc<Mutex<VecDeque<Vec<u8>>>>,
    }

    impl InputReportReader for FakeInputReader {
        fn read_report(
            &mut self,
            _timeout: Duration,
        ) -> Result<Option<HostInputReport>, HostHidError> {
            Ok(self
                .reports
                .lock()
                .unwrap()
                .pop_front()
                .map(|data| HostInputReport {
                    descriptor: InputDescriptor {
                        interface_number: 2,
                        role: CollectionRole::PuckMain,
                    },
                    data,
                }))
        }
    }

    #[test]
    fn host_session_serves_shared_mock_guest_input_and_commands() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let backend = FakeBackend::new(vec![vec![0x79, 0x02]]);
        let backend_for_server = backend.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            let hello = control.read_frame().unwrap();
            handle_session_with_backend(
                &listeners,
                &mut control,
                SessionStart {
                    session_id: 1,
                    session_trace_id: 0x12345,
                    guest_log_level_override: Some(LogSeverity::Debug),
                    hello_request_id: hello.header.id,
                    guest_pid: 1234,
                },
                &AppState::new(),
                Arc::new(backend_for_server),
                &server_stop,
            )
            .unwrap();
        });

        ready_rx.recv().unwrap();
        let mut guest = GuestTransportClient::connect(GuestTransportConfig {
            addrs,
            guest_label: "host-runtime-test".to_string(),
            ..GuestTransportConfig::default()
        })
        .unwrap();

        assert_eq!(guest.identity().serial, "FXB9961303C9C");
        assert_eq!(guest.session_trace_id(), 0x12345);
        assert_eq!(guest.guest_log_level_override(), Some(LogSeverity::Debug));
        assert_eq!(guest.read_input_report().unwrap().data, vec![0x79, 0x02]);
        assert_eq!(
            guest.get_feature(2, 0x02, 64, 100).unwrap().data,
            vec![0x02, 0xB4]
        );
        assert_eq!(
            guest
                .write_report(2, 100, &[0x00, 0x80, 0xAA, 0xBB])
                .unwrap()
                .bytes_written,
            4
        );
        assert_eq!(
            guest
                .set_feature(2, 100, &[0x01, 0x83, 0x01, 0x00])
                .unwrap()
                .bytes_accepted,
            4
        );
        assert_eq!(
            guest
                .set_output(2, 100, &[0x80, 0x00, 0x00, 0x00])
                .unwrap()
                .bytes_accepted,
            4
        );

        drop(guest);
        server.join().unwrap();

        assert_eq!(
            backend.operations(),
            vec![
                "get_feature:2:02:64",
                "write:[00, 80, AA, BB]",
                "set_feature:[01, 83, 01, 00]",
                "set_output:[80, 00, 00, 00]",
            ]
        );
        assert!(backend.cleanup_called());
    }

    #[test]
    fn host_runtime_accepts_reconnect_with_shared_guest_runtime() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            for session_id in 1..=2 {
                let backend = FakeBackend::new(Vec::new());
                let mut control = listeners.accept_control().unwrap();
                let hello = control.read_frame().unwrap();
                handle_session_with_backend(
                    &listeners,
                    &mut control,
                    SessionStart {
                        session_id,
                        session_trace_id: 0x12345 + session_id,
                        guest_log_level_override: None,
                        hello_request_id: hello.header.id,
                        guest_pid: 1234,
                    },
                    &AppState::new(),
                    Arc::new(backend),
                    &server_stop,
                )
                .unwrap();
            }
        });

        ready_rx.recv().unwrap();
        for _ in 0..2 {
            let guest = GuestTransportClient::connect(GuestTransportConfig {
                addrs,
                guest_label: "host-reconnect-test".to_string(),
                ..GuestTransportConfig::default()
            })
            .unwrap();
            assert_eq!(guest.identity().serial, "FXB9961303C9C");
            drop(guest);
        }

        server.join().unwrap();
    }

    #[test]
    fn host_session_rejects_control_message_on_input_channel() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let backend = FakeBackend::new(Vec::new());
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            let hello = control.read_frame().unwrap();
            let result = handle_session_with_backend(
                &listeners,
                &mut control,
                SessionStart {
                    session_id: 1,
                    session_trace_id: 0x12345,
                    guest_log_level_override: None,
                    hello_request_id: hello.header.id,
                    guest_pid: 1234,
                },
                &AppState::new(),
                Arc::new(backend),
                &server_stop,
            );
            result_tx
                .send(matches!(
                    result,
                    Err(RuntimeError::UnexpectedMessage {
                        expected: MessageType::InputAttach,
                        actual: MessageType::Hello,
                    })
                ))
                .unwrap();
        });

        ready_rx.recv().unwrap();
        let mut control = ChannelStream::connect(Channel::Control, addrs.control).unwrap();
        write_payload(&mut control, 1, &Hello::new(1234)).unwrap();
        assert_eq!(
            control.read_frame().unwrap().header.message_type,
            MessageType::HelloOk
        );
        assert_eq!(
            control.read_frame().unwrap().header.message_type,
            MessageType::Identity
        );

        let mut input = ChannelStream::connect(Channel::Input, addrs.input).unwrap();
        write_payload(&mut input, 2, &Hello::new(1234)).unwrap();
        drop(input);
        drop(control);

        assert!(result_rx.recv().unwrap());
        server.join().unwrap();
    }

    #[test]
    fn host_session_times_out_missing_input_attach() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let backend = FakeBackend::new(Vec::new());
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            let hello = control.read_frame().unwrap();
            listeners.set_nonblocking(true).unwrap();
            let result = handle_session_with_backend_timeout(
                &listeners,
                &mut control,
                SessionStart {
                    session_id: 1,
                    session_trace_id: 0x12345,
                    guest_log_level_override: None,
                    hello_request_id: hello.header.id,
                    guest_pid: 1234,
                },
                &AppState::new(),
                Arc::new(backend),
                &server_stop,
                Duration::from_millis(100),
            );
            result_tx
                .send(matches!(result, Err(RuntimeError::InputAttachTimeout(_))))
                .unwrap();
        });

        ready_rx.recv().unwrap();
        let mut control = ChannelStream::connect(Channel::Control, addrs.control).unwrap();
        write_payload(&mut control, 1, &Hello::new(1234)).unwrap();
        assert_eq!(
            control.read_frame().unwrap().header.message_type,
            MessageType::HelloOk
        );
        assert_eq!(
            control.read_frame().unwrap().header.message_type,
            MessageType::Identity
        );
        drop(control);

        assert!(result_rx.recv().unwrap());
        server.join().unwrap();
    }

    #[test]
    fn host_session_rejects_bad_protocol_version_before_hid_probe() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            let result = handle_session(
                &listeners,
                &mut control,
                1,
                0x12345,
                None,
                &AppState::new(),
                &server_stop,
            );
            result_tx
                .send(matches!(
                    result,
                    Err(RuntimeError::ProtocolVersionMismatch(999))
                ))
                .unwrap();
        });

        ready_rx.recv().unwrap();
        let mut control = ChannelStream::connect(Channel::Control, addrs.control).unwrap();
        write_payload(
            &mut control,
            1,
            &Hello {
                guest_pid: 1234,
                guest_protocol_version: 999,
                guest_capabilities: 0,
            },
        )
        .unwrap();

        let response = control.read_frame().unwrap();
        assert_eq!(response.header.message_type, MessageType::HelloOk);
        assert_eq!(response.header.id, 1);
        let hello_ok = HelloOk::decode(&response.payload).unwrap();
        assert_eq!(hello_ok.status, StatusCode::ProtocolError);

        assert!(result_rx.recv().unwrap());
        server.join().unwrap();
    }

    #[test]
    fn host_service_shutdown_stops_idle_listener() {
        let app_state = AppState::new();
        let service = start_host_service_on(app_state.clone(), TransportAddrs::loopback(0, 0));

        let state = wait_for_state(&app_state, |state| {
            !matches!(state, HostRuntimeState::Starting)
        });
        assert!(!matches!(state, HostRuntimeState::Starting));

        service.shutdown();
        assert_eq!(app_state.snapshot(), HostRuntimeState::Stopping);
    }

    #[test]
    fn host_service_reports_degraded_on_port_conflict() {
        let guard = TcpListener::bind("127.0.0.1:0").unwrap();
        let app_state = AppState::new();
        let service = start_host_service_on(
            app_state.clone(),
            TransportAddrs::loopback(guard.local_addr().unwrap().port(), 0),
        );

        let state = wait_for_state(&app_state, |state| {
            matches!(state, HostRuntimeState::Degraded { .. })
        });
        assert!(
            matches!(state, HostRuntimeState::Degraded { reason } if reason.contains("listen failed"))
        );

        service.shutdown();
        assert_eq!(app_state.snapshot(), HostRuntimeState::Stopping);
    }

    fn test_identity() -> IdentityPayload {
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

    fn wait_for_state(
        app_state: &AppState,
        predicate: impl Fn(&HostRuntimeState) -> bool,
    ) -> HostRuntimeState {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let state = app_state.snapshot();
            if predicate(&state) || Instant::now() >= deadline {
                return state;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
