use crosspuck_core::guest::{
    GuestControl, GuestError, GuestInput, GuestSessionInfo, GuestTransportClient,
    GuestTransportConfig,
};
use crosspuck_core::protocol::{FeatureResult, QueuedInputReport, StatusCode};
use crosspuck_core::transport::TransportAddrs;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_INPUT_QUEUE_CAPACITY: usize = 64;
const DEFAULT_GUEST_IO_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct HostBridgeConfig {
    pub addrs: TransportAddrs,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub guest_label: String,
    pub input_queue_capacity: usize,
}

impl Default for HostBridgeConfig {
    fn default() -> Self {
        Self {
            addrs: TransportAddrs::default(),
            connect_timeout: Duration::from_secs(2),
            io_timeout: DEFAULT_GUEST_IO_TIMEOUT,
            guest_label: "crosspuck-hid-proxy".to_string(),
            input_queue_capacity: DEFAULT_INPUT_QUEUE_CAPACITY,
        }
    }
}

pub struct HostBridge {
    info: GuestSessionInfo,
    control: Mutex<GuestControl>,
    input_queue: Arc<Mutex<VecDeque<QueuedInputReport>>>,
    stats: Arc<Mutex<HostBridgeInputStats>>,
    running: Arc<AtomicBool>,
    input_thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostBridgeInputStats {
    pub pushed: u64,
    pub popped: u64,
    pub dropped_oldest: u64,
    pub read_errors: u64,
}

impl HostBridge {
    pub fn connect(config: HostBridgeConfig) -> Result<Self, HostBridgeError> {
        let session = GuestTransportClient::connect(GuestTransportConfig {
            addrs: config.addrs,
            connect_timeout: config.connect_timeout,
            io_timeout: config.io_timeout,
            guest_label: config.guest_label,
            ..GuestTransportConfig::default()
        })?;
        let parts = session.into_parts();
        let input_queue = Arc::new(Mutex::new(VecDeque::with_capacity(
            config.input_queue_capacity,
        )));
        let stats = Arc::new(Mutex::new(HostBridgeInputStats::default()));
        let running = Arc::new(AtomicBool::new(true));
        let input_thread = spawn_input_pump(
            parts.input,
            Arc::clone(&input_queue),
            Arc::clone(&stats),
            Arc::clone(&running),
            config.input_queue_capacity.max(1),
        );

        Ok(Self {
            info: parts.info,
            control: Mutex::new(parts.control),
            input_queue,
            stats,
            running,
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
        let Some(index) = queue
            .iter()
            .position(|report| report.interface_number == interface_number)
        else {
            return Ok(None);
        };
        let report = queue.remove(index);
        drop(queue);

        if report.is_some() {
            self.with_stats(|stats| stats.popped += 1);
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
        let mut control = self
            .control
            .lock()
            .map_err(|_| HostBridgeError::ControlPoisoned)?;
        let result = control.get_feature(interface_number, report_id, requested_len, timeout_ms)?;
        ensure_status("GET_FEATURE", result.status, result.os_error)?;
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
        let mut control = self
            .control
            .lock()
            .map_err(|_| HostBridgeError::ControlPoisoned)?;
        let result = control.set_feature(interface_number, timeout_ms, payload)?;
        ensure_status("SET_FEATURE", result.status, result.os_error)?;
        Ok(result.bytes_accepted)
    }

    pub fn set_output(
        &self,
        interface_number: u8,
        payload: &[u8],
        timeout_ms: u16,
    ) -> Result<u16, HostBridgeError> {
        let mut control = self
            .control
            .lock()
            .map_err(|_| HostBridgeError::ControlPoisoned)?;
        let result = control.set_output(interface_number, timeout_ms, payload)?;
        ensure_status("SET_OUTPUT", result.status, result.os_error)?;
        Ok(result.bytes_accepted)
    }

    pub fn write_report(
        &self,
        interface_number: u8,
        payload: &[u8],
        timeout_ms: u16,
    ) -> Result<u16, HostBridgeError> {
        let mut control = self
            .control
            .lock()
            .map_err(|_| HostBridgeError::ControlPoisoned)?;
        let result = control.write_report(interface_number, timeout_ms, payload)?;
        ensure_status("WRITE", result.status, result.os_error)?;
        Ok(result.bytes_written)
    }

    fn with_stats(&self, update: impl FnOnce(&mut HostBridgeInputStats)) {
        if let Ok(mut stats) = self.stats.lock() {
            update(&mut stats);
        }
    }
}

impl Drop for HostBridge {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Ok(mut input_thread) = self.input_thread.lock() {
            if let Some(input_thread) = input_thread.take() {
                let _ = input_thread.join();
            }
        }
    }
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
    ControlPoisoned,
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
            Self::ControlPoisoned => f.write_str("host bridge control lock poisoned"),
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
            Self::Guest(_) | Self::ControlPoisoned | Self::QueuePoisoned
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

impl From<GuestError> for HostBridgeError {
    fn from(value: GuestError) -> Self {
        Self::Guest(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crosspuck_core::protocol::{
        CollectionDescriptor, CollectionRole, FeatureResult, HelloOk, IdentityPayload, InputAttach,
        InputAttachOk, InputReport, MessageType, SetFeature, SetFeatureResult, SetOutput,
        SetOutputResult, StatusCode, WireDecode, WirePayload, WriteReport, WriteResult,
    };
    use crosspuck_core::transport::{TransportAddrs, TransportListeners};
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

    fn write_payload<T: WirePayload>(
        stream: &mut crosspuck_core::transport::ChannelStream,
        id: u32,
        payload: &T,
    ) {
        stream
            .write_frame(&crosspuck_core::protocol::Frame::new(
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
