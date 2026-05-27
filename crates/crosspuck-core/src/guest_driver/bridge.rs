use crate::guest::{
    GuestControl, GuestError, GuestInput, GuestSessionInfo, GuestTransportClient,
    GuestTransportConfig,
};
use crate::protocol::{CollectionRole, FeatureResult, QueuedInputReport, StatusCode};
use crate::transport::TransportAddrs;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_INPUT_QUEUE_CAPACITY: usize = 64;
const DEFAULT_FEEDBACK_QUEUE_CAPACITY: usize = 64;
const DEFAULT_GUEST_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_GUEST_IO_TIMEOUT: Duration = Duration::from_millis(50);
const INPUT_PUMP_READ_TIMEOUT: Duration = Duration::from_millis(50);
const PUCK_MAIN_INTERFACE: u8 = 2;
const COMMAND_WORKER_WAKE_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct HostBridgeConfig {
    pub addrs: TransportAddrs,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
    pub io_timeout: Duration,
    pub guest_label: String,
    pub input_queue_capacity: usize,
}

impl Default for HostBridgeConfig {
    fn default() -> Self {
        Self {
            addrs: TransportAddrs::default(),
            connect_timeout: Duration::from_secs(2),
            handshake_timeout: DEFAULT_GUEST_HANDSHAKE_TIMEOUT,
            io_timeout: DEFAULT_GUEST_IO_TIMEOUT,
            guest_label: "crosspuck-driver".to_string(),
            input_queue_capacity: DEFAULT_INPUT_QUEUE_CAPACITY,
        }
    }
}

pub struct HostBridge {
    info: GuestSessionInfo,
    control_transport_timeout_ms: u16,
    command_queue: CommandQueue,
    input_queue: Arc<Mutex<VecDeque<QueuedInputReport>>>,
    input_routes: Arc<Mutex<InputRouteState>>,
    stats: Arc<Mutex<HostBridgeInputStats>>,
    running: Arc<AtomicBool>,
    command_thread: Mutex<Option<JoinHandle<()>>>,
    input_thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostBridgeInputStats {
    pub pushed: u64,
    pub popped: u64,
    pub dropped_oldest: u64,
    pub dropped_stale: u64,
    pub read_errors: u64,
    pub feedback_enqueued: u64,
    pub feedback_sent: u64,
    pub feedback_dropped: u64,
    pub command_errors: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct InputRouteState {
    preferred_wireless_interface: Option<u8>,
}

type CommandQueue = Arc<(Mutex<VecDeque<CommandRequest>>, Condvar)>;
type FeatureResponse = mpsc::SyncSender<Result<FeatureResult, HostBridgeError>>;
type SetFeatureResponse = mpsc::SyncSender<Result<u16, HostBridgeError>>;

enum CommandRequest {
    GetFeature {
        interface_number: u8,
        report_id: u8,
        requested_len: u16,
        timeout_ms: u16,
        transport_timeout_ms: u16,
        response: FeatureResponse,
    },
    SetFeature {
        interface_number: u8,
        payload: Vec<u8>,
        timeout_ms: u16,
        transport_timeout_ms: u16,
        response: SetFeatureResponse,
    },
    SetOutput {
        interface_number: u8,
        payload: Vec<u8>,
        timeout_ms: u16,
        transport_timeout_ms: u16,
    },
    Write {
        interface_number: u8,
        payload: Vec<u8>,
        timeout_ms: u16,
        transport_timeout_ms: u16,
    },
}

impl CommandRequest {
    fn is_feedback(&self) -> bool {
        matches!(self, Self::SetOutput { .. } | Self::Write { .. })
    }

    fn respond_stopped(self) {
        match self {
            Self::GetFeature { response, .. } => {
                let _ = response.send(Err(HostBridgeError::CommandWorkerStopped));
            }
            Self::SetFeature { response, .. } => {
                let _ = response.send(Err(HostBridgeError::CommandWorkerStopped));
            }
            Self::SetOutput { .. } | Self::Write { .. } => {}
        }
    }
}

impl HostBridge {
    pub fn connect(config: HostBridgeConfig) -> Result<Self, HostBridgeError> {
        let session = GuestTransportClient::connect(GuestTransportConfig {
            addrs: config.addrs,
            connect_timeout: config.connect_timeout,
            handshake_timeout: config.handshake_timeout,
            io_timeout: config.io_timeout,
            guest_label: config.guest_label,
            ..GuestTransportConfig::default()
        })?;
        let parts = session.into_parts();
        let effective_io_timeout = parts
            .info
            .guest_runtime_overrides
            .io_timeout_ms
            .map(|millis| Duration::from_millis(u64::from(millis)))
            .unwrap_or(config.io_timeout);
        let input_queue_capacity = parts
            .info
            .guest_runtime_overrides
            .input_queue_capacity
            .map(usize::from)
            .unwrap_or(config.input_queue_capacity)
            .max(1);
        parts
            .input
            .set_read_timeout(Some(INPUT_PUMP_READ_TIMEOUT))?;
        let input_queue = Arc::new(Mutex::new(VecDeque::with_capacity(input_queue_capacity)));
        let input_routes = Arc::new(Mutex::new(InputRouteState::default()));
        let stats = Arc::new(Mutex::new(HostBridgeInputStats::default()));
        let running = Arc::new(AtomicBool::new(true));
        let command_queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let command_thread = spawn_command_worker(
            parts.control,
            Arc::clone(&command_queue),
            Arc::clone(&stats),
            Arc::clone(&running),
        );
        let input_thread = spawn_input_pump(
            parts.input,
            Arc::clone(&input_queue),
            Arc::clone(&stats),
            Arc::clone(&running),
            input_queue_capacity,
        );

        Ok(Self {
            info: parts.info,
            control_transport_timeout_ms: duration_to_u16_ms(effective_io_timeout),
            command_queue,
            input_queue,
            input_routes,
            stats,
            running,
            command_thread: Mutex::new(Some(command_thread)),
            input_thread: Mutex::new(Some(input_thread)),
        })
    }

    pub fn info(&self) -> &GuestSessionInfo {
        &self.info
    }

    pub fn input_stats(&self) -> HostBridgeInputStats {
        self.stats.lock().map(|stats| *stats).unwrap_or_default()
    }

    pub fn pop_input_report(
        &self,
        interface_number: u8,
    ) -> Result<Option<QueuedInputReport>, HostBridgeError> {
        let mut queue = self
            .input_queue
            .lock()
            .map_err(|_| HostBridgeError::QueuePoisoned)?;
        let routes = self
            .input_routes
            .lock()
            .map(|routes| *routes)
            .unwrap_or_default();
        let Some((index, routed_interface)) =
            find_latest_report_for_interface(&queue, interface_number, routes)
        else {
            return Ok(None);
        };
        let mut report = queue.remove(index);
        let dropped_stale = drop_stale_reports_for_interface(
            &mut queue,
            interface_number,
            routed_interface,
            routes,
        );
        drop(queue);

        if let Some(report) = report.as_mut() {
            if let Some(routed_interface) = routed_interface {
                report.interface_number = routed_interface;
                if let Some(role) = wireless_role_for_interface(routed_interface) {
                    report.role = role;
                }
            }
            self.with_stats(|stats| {
                stats.popped += 1;
                stats.dropped_stale += dropped_stale as u64;
            });
        }
        Ok(report)
    }

    pub fn copy_next_input_report(
        &self,
        interface_number: u8,
        output: &mut [u8],
    ) -> Result<Option<usize>, HostBridgeError> {
        let Some(report) = self.pop_input_report(interface_number)? else {
            return Ok(None);
        };
        output.fill(0);
        let count = output.len().min(report.data.len());
        output[..count].copy_from_slice(&report.data[..count]);
        Ok(Some(count))
    }

    pub fn get_feature_report(
        &self,
        interface_number: u8,
        report_id: u8,
        requested_len: u16,
        timeout_ms: u16,
    ) -> Result<FeatureResult, HostBridgeError> {
        let (response, result) = mpsc::sync_channel(1);
        self.enqueue_command(CommandRequest::GetFeature {
            interface_number,
            report_id,
            requested_len,
            timeout_ms,
            transport_timeout_ms: self.control_transport_timeout_ms(),
            response,
        })?;
        let result = result
            .recv()
            .map_err(|_| HostBridgeError::CommandWorkerStopped)??;
        self.update_input_routes_from_feature(interface_number, report_id, &result.data);
        Ok(result)
    }

    pub fn copy_feature_report(
        &self,
        interface_number: u8,
        report_id: u8,
        output: &mut [u8],
        timeout_ms: u16,
    ) -> Result<usize, HostBridgeError> {
        let result = self.get_feature_report(
            interface_number,
            report_id,
            output.len().min(u16::MAX as usize) as u16,
            timeout_ms,
        )?;
        output.fill(0);
        let count = output.len().min(result.data.len());
        output[..count].copy_from_slice(&result.data[..count]);
        Ok(count)
    }

    pub fn set_feature(
        &self,
        interface_number: u8,
        payload: &[u8],
        timeout_ms: u16,
    ) -> Result<u16, HostBridgeError> {
        let (response, result) = mpsc::sync_channel(1);
        self.enqueue_command(CommandRequest::SetFeature {
            interface_number,
            payload: payload.to_vec(),
            timeout_ms,
            transport_timeout_ms: self.control_transport_timeout_ms(),
            response,
        })?;
        result
            .recv()
            .map_err(|_| HostBridgeError::CommandWorkerStopped)?
    }

    pub fn set_output(
        &self,
        interface_number: u8,
        payload: &[u8],
        timeout_ms: u16,
    ) -> Result<u16, HostBridgeError> {
        self.enqueue_feedback(CommandRequest::SetOutput {
            interface_number,
            payload: payload.to_vec(),
            timeout_ms,
            transport_timeout_ms: self.control_transport_timeout_ms(),
        })?;
        Ok(saturating_u16(payload.len()))
    }

    pub fn write_report(
        &self,
        interface_number: u8,
        payload: &[u8],
        timeout_ms: u16,
    ) -> Result<u16, HostBridgeError> {
        self.enqueue_feedback(CommandRequest::Write {
            interface_number,
            payload: payload.to_vec(),
            timeout_ms,
            transport_timeout_ms: self.control_transport_timeout_ms(),
        })?;
        Ok(saturating_u16(payload.len()))
    }

    fn with_stats(&self, update: impl FnOnce(&mut HostBridgeInputStats)) {
        if let Ok(mut stats) = self.stats.lock() {
            update(&mut stats);
        }
    }

    fn control_transport_timeout_ms(&self) -> u16 {
        self.control_transport_timeout_ms
    }

    fn enqueue_command(&self, command: CommandRequest) -> Result<(), HostBridgeError> {
        if !self.running.load(Ordering::Relaxed) {
            return Err(HostBridgeError::CommandWorkerStopped);
        }
        let (queue, wake) = &*self.command_queue;
        let mut queue = queue
            .lock()
            .map_err(|_| HostBridgeError::CommandQueuePoisoned)?;
        if !self.running.load(Ordering::Relaxed) {
            return Err(HostBridgeError::CommandWorkerStopped);
        }
        queue.push_back(command);
        wake.notify_one();
        Ok(())
    }

    fn enqueue_feedback(&self, command: CommandRequest) -> Result<(), HostBridgeError> {
        if !command.is_feedback() {
            return Err(HostBridgeError::BadRequest("non-feedback command"));
        }
        if !self.running.load(Ordering::Relaxed) {
            return Err(HostBridgeError::CommandWorkerStopped);
        }

        let (queue, wake) = &*self.command_queue;
        let mut queue = queue
            .lock()
            .map_err(|_| HostBridgeError::CommandQueuePoisoned)?;
        if !self.running.load(Ordering::Relaxed) {
            return Err(HostBridgeError::CommandWorkerStopped);
        }
        let feedback_count = queue.iter().filter(|command| command.is_feedback()).count();
        if feedback_count >= DEFAULT_FEEDBACK_QUEUE_CAPACITY {
            if let Some(index) = queue.iter().position(|command| command.is_feedback()) {
                queue.remove(index);
                self.with_stats(|stats| stats.feedback_dropped += 1);
            }
        }
        queue.push_back(command);
        self.with_stats(|stats| stats.feedback_enqueued += 1);
        wake.notify_one();
        Ok(())
    }
}

fn find_latest_report_for_interface(
    queue: &VecDeque<QueuedInputReport>,
    interface_number: u8,
    routes: InputRouteState,
) -> Option<(usize, Option<u8>)> {
    let exact = queue.iter().rposition(|report| {
        report.interface_number == interface_number
            && !should_hold_main_report_for_wireless(report, interface_number, routes)
    });
    if let Some(index) = exact {
        return Some((index, None));
    }

    if wireless_role_for_interface(interface_number).is_some()
        && routes
            .preferred_wireless_interface
            .is_none_or(|preferred| preferred == interface_number)
    {
        let index = queue.iter().rposition(|report| {
            report.interface_number == PUCK_MAIN_INTERFACE && is_triton_input_report(&report.data)
        })?;
        return Some((index, Some(interface_number)));
    }

    None
}

fn drop_stale_reports_for_interface(
    queue: &mut VecDeque<QueuedInputReport>,
    interface_number: u8,
    routed_interface: Option<u8>,
    routes: InputRouteState,
) -> usize {
    let before = queue.len();
    if routed_interface.is_some() {
        queue.retain(|report| {
            !(report.interface_number == PUCK_MAIN_INTERFACE
                && is_triton_input_report(&report.data))
        });
    } else {
        queue.retain(|report| {
            !(report.interface_number == interface_number
                && !should_hold_main_report_for_wireless(report, interface_number, routes))
        });
    }
    before.saturating_sub(queue.len())
}

fn should_hold_main_report_for_wireless(
    report: &QueuedInputReport,
    requested_interface: u8,
    routes: InputRouteState,
) -> bool {
    requested_interface == PUCK_MAIN_INTERFACE
        && routes.preferred_wireless_interface.is_some()
        && report.interface_number == PUCK_MAIN_INTERFACE
        && is_triton_input_report(&report.data)
}

fn wireless_role_for_interface(interface_number: u8) -> Option<CollectionRole> {
    match interface_number {
        3 => Some(CollectionRole::PuckInterface3),
        4 => Some(CollectionRole::PuckInterface4),
        5 => Some(CollectionRole::PuckInterface5),
        _ => None,
    }
}

fn is_triton_input_report(data: &[u8]) -> bool {
    matches!(
        data.first().copied(),
        Some(0x42 | 0x43 | 0x45 | 0x46 | 0x79)
    )
}

impl HostBridge {
    fn update_input_routes_from_feature(&self, interface_number: u8, report_id: u8, data: &[u8]) {
        if report_id != 0x02
            || data.get(1).copied() != Some(0xA3)
            || wireless_role_for_interface(interface_number).is_none()
        {
            return;
        }

        let registered = data.iter().skip(3).any(|byte| *byte != 0);
        if let Ok(mut routes) = self.input_routes.lock() {
            if registered {
                routes.preferred_wireless_interface = Some(interface_number);
            } else if routes.preferred_wireless_interface == Some(interface_number) {
                routes.preferred_wireless_interface = None;
            }
        }
    }
}

impl Drop for HostBridge {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let (_, wake) = &*self.command_queue;
        wake.notify_all();
        if let Ok(mut command_thread) = self.command_thread.lock() {
            if let Some(command_thread) = command_thread.take() {
                let _ = command_thread.join();
            }
        }
        if let Ok(mut input_thread) = self.input_thread.lock() {
            if let Some(input_thread) = input_thread.take() {
                let _ = input_thread.join();
            }
        }
    }
}

fn spawn_command_worker(
    mut control: GuestControl,
    queue: CommandQueue,
    stats: Arc<Mutex<HostBridgeInputStats>>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            let Some(command) = wait_for_command(&queue, &running) else {
                return;
            };
            let result = process_command(&mut control, command, &stats);
            if result.is_err_and(|error| error.should_disconnect_bridge()) {
                stop_command_worker(&queue, &running);
                return;
            }
        }
    })
}

fn wait_for_command(queue: &CommandQueue, running: &Arc<AtomicBool>) -> Option<CommandRequest> {
    let (queue, wake) = &**queue;
    let mut queue = queue.lock().ok()?;
    loop {
        if let Some(command) = queue.pop_front() {
            return Some(command);
        }
        if !running.load(Ordering::Relaxed) {
            return None;
        }
        let (next_queue, _) = wake
            .wait_timeout(queue, COMMAND_WORKER_WAKE_INTERVAL)
            .ok()?;
        queue = next_queue;
    }
}

fn stop_command_worker(queue: &CommandQueue, running: &Arc<AtomicBool>) {
    running.store(false, Ordering::Relaxed);
    let (queue, wake) = &**queue;
    if let Ok(mut queue) = queue.lock() {
        for command in queue.drain(..) {
            command.respond_stopped();
        }
    }
    wake.notify_all();
}

fn process_command(
    control: &mut GuestControl,
    command: CommandRequest,
    stats: &Arc<Mutex<HostBridgeInputStats>>,
) -> Result<(), HostBridgeError> {
    match command {
        CommandRequest::GetFeature {
            interface_number,
            report_id,
            requested_len,
            timeout_ms,
            transport_timeout_ms,
            response,
        } => {
            let result = control
                .get_feature_with_transport_timeout(
                    interface_number,
                    report_id,
                    requested_len,
                    timeout_ms,
                    transport_timeout_ms,
                )
                .map_err(HostBridgeError::from)
                .and_then(|result| {
                    ensure_status("GET_FEATURE", result.status, result.os_error)?;
                    Ok(result)
                });
            let should_disconnect = result
                .as_ref()
                .err()
                .is_some_and(HostBridgeError::should_disconnect_bridge);
            let _ = response.send(result);
            if should_disconnect {
                return Err(HostBridgeError::CommandWorkerStopped);
            }
        }
        CommandRequest::SetFeature {
            interface_number,
            payload,
            timeout_ms,
            transport_timeout_ms,
            response,
        } => {
            let result = control
                .set_feature_with_transport_timeout(
                    interface_number,
                    timeout_ms,
                    &payload,
                    transport_timeout_ms,
                )
                .map_err(HostBridgeError::from)
                .and_then(|result| {
                    ensure_status("SET_FEATURE", result.status, result.os_error)?;
                    Ok(result.bytes_accepted)
                });
            let should_disconnect = result
                .as_ref()
                .err()
                .is_some_and(HostBridgeError::should_disconnect_bridge);
            let _ = response.send(result);
            if should_disconnect {
                return Err(HostBridgeError::CommandWorkerStopped);
            }
        }
        CommandRequest::SetOutput {
            interface_number,
            payload,
            timeout_ms,
            transport_timeout_ms,
        } => {
            let result = control
                .set_output_with_transport_timeout(
                    interface_number,
                    timeout_ms,
                    &payload,
                    transport_timeout_ms,
                )
                .map_err(HostBridgeError::from)
                .and_then(|result| {
                    ensure_status("SET_OUTPUT", result.status, result.os_error)?;
                    Ok(result.bytes_accepted)
                });
            match result {
                Ok(_) => with_stats(stats, |stats| stats.feedback_sent += 1),
                Err(error) => {
                    if error.should_disconnect_bridge() {
                        with_stats(stats, |stats| stats.command_errors += 1);
                        return Err(error);
                    }
                }
            }
        }
        CommandRequest::Write {
            interface_number,
            payload,
            timeout_ms,
            transport_timeout_ms,
        } => {
            let result = control
                .write_report_with_transport_timeout(
                    interface_number,
                    timeout_ms,
                    &payload,
                    transport_timeout_ms,
                )
                .map_err(HostBridgeError::from)
                .and_then(|result| {
                    ensure_status("WRITE", result.status, result.os_error)?;
                    Ok(result.bytes_written)
                });
            match result {
                Ok(_) => with_stats(stats, |stats| stats.feedback_sent += 1),
                Err(error) => {
                    if error.should_disconnect_bridge() {
                        with_stats(stats, |stats| stats.command_errors += 1);
                        return Err(error);
                    }
                }
            }
        }
    }
    Ok(())
}

fn spawn_input_pump(
    mut input: GuestInput,
    queue: Arc<Mutex<VecDeque<QueuedInputReport>>>,
    stats: Arc<Mutex<HostBridgeInputStats>>,
    running: Arc<AtomicBool>,
    capacity: usize,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            match input.read_input_report() {
                Ok(report) => {
                    let Ok(mut queue) = queue.lock() else {
                        return;
                    };
                    if queue.len() >= capacity {
                        queue.pop_front();
                        with_stats(&stats, |stats| stats.dropped_oldest += 1);
                    }
                    queue.push_back(report);
                    with_stats(&stats, |stats| stats.pushed += 1);
                }
                Err(error) if error.is_timeout_or_would_block() => {}
                Err(_) => {
                    with_stats(&stats, |stats| stats.read_errors += 1);
                    return;
                }
            }
        }
    })
}

fn ensure_status(
    operation: &'static str,
    status: StatusCode,
    os_error: u32,
) -> Result<(), HostBridgeError> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(HostBridgeError::NonOkStatus {
            operation,
            status,
            os_error,
        })
    }
}

fn with_stats(
    stats: &Arc<Mutex<HostBridgeInputStats>>,
    update: impl FnOnce(&mut HostBridgeInputStats),
) {
    if let Ok(mut stats) = stats.lock() {
        update(&mut stats);
    }
}

#[derive(Debug)]
pub enum HostBridgeError {
    Guest(GuestError),
    BadRequest(&'static str),
    CommandQueuePoisoned,
    CommandWorkerStopped,
    QueuePoisoned,
    NonOkStatus {
        operation: &'static str,
        status: StatusCode,
        os_error: u32,
    },
}

impl fmt::Display for HostBridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Guest(error) => write!(f, "{error}"),
            Self::BadRequest(message) => write!(f, "host bridge bad request: {message}"),
            Self::CommandQueuePoisoned => f.write_str("host bridge command queue lock poisoned"),
            Self::CommandWorkerStopped => f.write_str("host bridge command worker stopped"),
            Self::QueuePoisoned => f.write_str("host bridge input queue lock poisoned"),
            Self::NonOkStatus {
                operation,
                status,
                os_error,
            } => write!(
                f,
                "host bridge {operation} failed: status={status} os_error={os_error}"
            ),
        }
    }
}

impl HostBridgeError {
    pub fn should_disconnect_bridge(&self) -> bool {
        matches!(
            self,
            Self::Guest(_)
                | Self::CommandQueuePoisoned
                | Self::CommandWorkerStopped
                | Self::QueuePoisoned
        )
    }
}

impl std::error::Error for HostBridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Guest(error) => Some(error),
            _ => None,
        }
    }
}

fn saturating_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn duration_to_u16_ms(value: Duration) -> u16 {
    value.as_millis().min(u128::from(u16::MAX)) as u16
}

impl From<GuestError> for HostBridgeError {
    fn from(value: GuestError) -> Self {
        Self::Guest(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        CollectionDescriptor, CollectionRole, FeatureResult, Frame, HelloOk, IdentityPayload,
        InputAttach, InputAttachOk, InputReport, MessageType, SetFeature, SetFeatureResult,
        SetOutput, SetOutputResult, WireDecode, WirePayload, WriteReport, WriteResult,
    };
    use crate::transport::{ChannelStream, TransportAddrs, TransportListeners};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn bridge_pumps_input_and_forwards_commands() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_payload(&mut control, 0, &identity());

            let mut input = listeners.accept_input().unwrap();
            input
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            input
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let attach = input.read_frame().unwrap();
            assert_eq!(attach.header.message_type, MessageType::InputAttach);
            let attach_payload = InputAttach::decode(&attach.payload).unwrap();
            assert_eq!(attach_payload.session_id, 0xCAFE_BABE);
            write_payload(
                &mut input,
                attach.header.id,
                &InputAttachOk {
                    status: StatusCode::Ok,
                    input_report_len: 54,
                    first_input_seq: 1,
                },
            );
            write_payload(
                &mut input,
                1,
                &InputReport {
                    interface_number: 2,
                    role: CollectionRole::PuckMain,
                    host_monotonic_us: 10,
                    data: vec![0x79, 0x02],
                },
            );

            let get_feature = control.read_frame().unwrap();
            assert_eq!(get_feature.header.message_type, MessageType::GetFeature);
            write_payload(
                &mut control,
                get_feature.header.id,
                &FeatureResult {
                    status: StatusCode::Ok,
                    os_error: 0,
                    data: vec![0x02, 0xB4, 0x01],
                },
            );

            let write = control.read_frame().unwrap();
            assert_eq!(write.header.message_type, MessageType::Write);
            let write_payload_data = WriteReport::decode(&write.payload).unwrap();
            assert_eq!(write_payload_data.data, vec![0x82, 0x03, 0x00, 0x00]);
            write_payload(
                &mut control,
                write.header.id,
                &WriteResult {
                    status: StatusCode::Ok,
                    bytes_written: write_payload_data.data.len() as u16,
                    os_error: 0,
                },
            );

            let set_feature = control.read_frame().unwrap();
            assert_eq!(set_feature.header.message_type, MessageType::SetFeature);
            let set_feature_payload = SetFeature::decode(&set_feature.payload).unwrap();
            assert_eq!(set_feature_payload.data, vec![0x02, 0xA3, 0x00]);
            write_payload(
                &mut control,
                set_feature.header.id,
                &SetFeatureResult {
                    status: StatusCode::Ok,
                    bytes_accepted: set_feature_payload.data.len() as u16,
                    os_error: 0,
                },
            );

            let set_output = control.read_frame().unwrap();
            assert_eq!(set_output.header.message_type, MessageType::SetOutput);
            let set_output_payload = SetOutput::decode(&set_output.payload).unwrap();
            assert_eq!(set_output_payload.data, vec![0x80, 0x00]);
            write_payload(
                &mut control,
                set_output.header.id,
                &SetOutputResult {
                    status: StatusCode::Ok,
                    bytes_accepted: set_output_payload.data.len() as u16,
                    os_error: 0,
                },
            );
        });

        ready_rx.recv().unwrap();
        let bridge = HostBridge::connect(HostBridgeConfig {
            addrs,
            io_timeout: Duration::from_millis(10),
            ..HostBridgeConfig::default()
        })
        .unwrap();
        assert_eq!(bridge.info().identity.serial, "FXB9961303C9C");

        let mut input = [0_u8; 64];
        let count = wait_for_input(&bridge, &mut input).unwrap();
        assert_eq!(count, 2);
        assert_eq!(&input[..2], &[0x79, 0x02]);

        let mut feature = [0_u8; 64];
        let feature_len = bridge
            .copy_feature_report(2, 0x02, &mut feature, 100)
            .unwrap();
        assert_eq!(feature_len, 3);
        assert_eq!(&feature[..3], &[0x02, 0xB4, 0x01]);
        assert_eq!(
            bridge
                .write_report(2, &[0x82, 0x03, 0x00, 0x00], 100)
                .unwrap(),
            4
        );
        assert_eq!(bridge.set_feature(2, &[0x02, 0xA3, 0x00], 100).unwrap(), 3);
        assert_eq!(bridge.set_output(2, &[0x80, 0x00], 100).unwrap(), 2);

        drop(bridge);
        server.join().unwrap();
    }

    #[test]
    fn bridge_routes_main_triton_reports_to_registered_wireless_interface() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_payload(&mut control, 0, &identity());

            let mut input = listeners.accept_input().unwrap();
            input
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            input
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
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
            );
            write_payload(
                &mut input,
                1,
                &InputReport {
                    interface_number: 2,
                    role: CollectionRole::PuckMain,
                    host_monotonic_us: 10,
                    data: vec![0x45, 0x83, 0x00, 0x00],
                },
            );

            let get_feature = control.read_frame().unwrap();
            assert_eq!(get_feature.header.message_type, MessageType::GetFeature);
            write_payload(
                &mut control,
                get_feature.header.id,
                &FeatureResult {
                    status: StatusCode::Ok,
                    os_error: 0,
                    data: vec![0x02, 0xA3, 0x18, 0xE5, 0xA0, b'F', b'X'],
                },
            );
        });

        ready_rx.recv().unwrap();
        let bridge = HostBridge::connect(HostBridgeConfig {
            addrs,
            io_timeout: Duration::from_millis(10),
            ..HostBridgeConfig::default()
        })
        .unwrap();

        let mut feature = [0_u8; 64];
        bridge
            .copy_feature_report(3, 0x02, &mut feature, 100)
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && bridge.input_stats().pushed == 0 {
            thread::sleep(Duration::from_millis(10));
        }

        let mut input = [0_u8; 64];
        assert_eq!(bridge.copy_next_input_report(2, &mut input).unwrap(), None);
        let count = bridge
            .copy_next_input_report(3, &mut input)
            .unwrap()
            .unwrap();
        assert_eq!(count, 4);
        assert_eq!(&input[..4], &[0x45, 0x83, 0x00, 0x00]);

        drop(bridge);
        server.join().unwrap();
    }

    #[test]
    fn bridge_returns_latest_input_report_and_drops_stale_matches() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_payload(&mut control, 0, &identity());

            let mut input = listeners.accept_input().unwrap();
            input
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            input
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
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
            );
            for sequence in 1..=3 {
                write_payload(
                    &mut input,
                    sequence,
                    &InputReport {
                        interface_number: 2,
                        role: CollectionRole::PuckMain,
                        host_monotonic_us: u64::from(sequence),
                        data: vec![0x79, sequence as u8],
                    },
                );
            }
        });

        ready_rx.recv().unwrap();
        let bridge = HostBridge::connect(HostBridgeConfig {
            addrs,
            io_timeout: Duration::from_millis(10),
            ..HostBridgeConfig::default()
        })
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && bridge.input_stats().pushed < 3 {
            thread::sleep(Duration::from_millis(1));
        }

        let mut input = [0_u8; 64];
        let count = bridge
            .copy_next_input_report(2, &mut input)
            .unwrap()
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(&input[..2], &[0x79, 0x03]);
        assert_eq!(bridge.input_stats().dropped_stale, 2);
        assert_eq!(bridge.copy_next_input_report(2, &mut input).unwrap(), None);

        drop(bridge);
        server.join().unwrap();
    }

    #[test]
    fn bridge_feedback_write_returns_before_host_response() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_payload(&mut control, 0, &identity());

            let mut input = listeners.accept_input().unwrap();
            input
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            input
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
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
            );

            let write = control.read_frame().unwrap();
            assert_eq!(write.header.message_type, MessageType::Write);
            let write_payload_data = WriteReport::decode(&write.payload).unwrap();
            thread::sleep(Duration::from_millis(120));
            write_payload(
                &mut control,
                write.header.id,
                &WriteResult {
                    status: StatusCode::Ok,
                    bytes_written: write_payload_data.data.len() as u16,
                    os_error: 0,
                },
            );

            let get_feature = control.read_frame().unwrap();
            assert_eq!(get_feature.header.message_type, MessageType::GetFeature);
            write_payload(
                &mut control,
                get_feature.header.id,
                &FeatureResult {
                    status: StatusCode::Ok,
                    os_error: 0,
                    data: vec![0x01, 0x00],
                },
            );
        });

        ready_rx.recv().unwrap();
        let bridge = HostBridge::connect(HostBridgeConfig {
            addrs,
            io_timeout: Duration::from_millis(250),
            ..HostBridgeConfig::default()
        })
        .unwrap();

        let started = Instant::now();
        assert_eq!(bridge.write_report(2, &[0x80, 0x00], 20).unwrap(), 2);
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "feedback write waited for the host response"
        );
        let mut feature = [0_u8; 64];
        assert_eq!(
            bridge
                .copy_feature_report(2, 0x01, &mut feature, 50)
                .unwrap(),
            2
        );
        assert_eq!(&feature[..2], &[0x01, 0x00]);

        drop(bridge);
        server.join().unwrap();
    }

    #[test]
    fn bridge_ignores_non_ok_async_feedback_without_poisoning_health() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_payload(&mut control, 0, &identity());

            let mut input = listeners.accept_input().unwrap();
            input
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            input
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
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
            );

            let write = control.read_frame().unwrap();
            assert_eq!(write.header.message_type, MessageType::Write);
            write_payload(
                &mut control,
                write.header.id,
                &WriteResult {
                    status: StatusCode::HidIoError,
                    bytes_written: 0,
                    os_error: 0,
                },
            );

            let get_feature = control.read_frame().unwrap();
            assert_eq!(get_feature.header.message_type, MessageType::GetFeature);
            write_payload(
                &mut control,
                get_feature.header.id,
                &FeatureResult {
                    status: StatusCode::Ok,
                    os_error: 0,
                    data: vec![0x01, 0x02, 0x03],
                },
            );
        });

        ready_rx.recv().unwrap();
        let bridge = HostBridge::connect(HostBridgeConfig {
            addrs,
            io_timeout: Duration::from_millis(250),
            ..HostBridgeConfig::default()
        })
        .unwrap();

        assert_eq!(bridge.write_report(2, &[0x80, 0x00], 20).unwrap(), 2);
        let mut feature = [0_u8; 64];
        assert_eq!(
            bridge
                .copy_feature_report(2, 0x01, &mut feature, 50)
                .unwrap(),
            3
        );
        assert_eq!(bridge.input_stats().command_errors, 0);

        drop(bridge);
        server.join().unwrap();
    }

    #[test]
    fn bridge_pending_sync_command_returns_when_feedback_worker_stops() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (close_tx, close_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut control = listeners.accept_control().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let hello = control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            write_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_payload(&mut control, 0, &identity());

            let mut input = listeners.accept_input().unwrap();
            input
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            input
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
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
            );

            let write = control.read_frame().unwrap();
            assert_eq!(write.header.message_type, MessageType::Write);
            close_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let bridge = HostBridge::connect(HostBridgeConfig {
            addrs,
            io_timeout: Duration::from_millis(250),
            ..HostBridgeConfig::default()
        })
        .unwrap();

        assert_eq!(bridge.write_report(2, &[0x80, 0x00], 250).unwrap(), 2);
        let close_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            close_tx.send(()).unwrap();
        });

        let mut feature = [0_u8; 64];
        let started = Instant::now();
        let result = bridge.copy_feature_report(2, 0x02, &mut feature, 250);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "pending feature command waited after feedback worker stopped"
        );
        assert!(matches!(result, Err(HostBridgeError::CommandWorkerStopped)));

        close_thread.join().unwrap();
        drop(bridge);
        server.join().unwrap();
    }

    fn wait_for_input(bridge: &HostBridge, output: &mut [u8]) -> Option<usize> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(count) = bridge.copy_next_input_report(2, output).unwrap() {
                return Some(count);
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn write_payload<T: WirePayload>(stream: &mut ChannelStream, id: u32, payload: &T) {
        stream
            .write_frame(&Frame::new(
                T::MESSAGE_TYPE,
                id,
                payload.to_bytes().unwrap(),
            ))
            .unwrap();
    }

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
}
