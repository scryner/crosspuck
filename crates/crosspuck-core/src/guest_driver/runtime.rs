use super::bridge::{HostBridge, HostBridgeError};
use super::config::{RuntimeConfig, DEFAULT_IO_TIMEOUT};
use super::handles::{VirtualHandleId, VirtualHandleTable};
use super::identity::{RuntimeIdentity, RuntimeIdentityState};
use super::profile::{VirtualHidProfile, VirtualHidProfileCatalog};
use super::trace::TraceLimiter;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_GET_FEATURE_TIMEOUT: Duration = Duration::from_millis(100);
const DEFAULT_SET_FEATURE_TIMEOUT: Duration = Duration::from_millis(50);
const DEFAULT_FEEDBACK_TIMEOUT: Duration = Duration::from_millis(20);

pub struct GuestDriverRuntime {
    config: RuntimeConfig,
    bridge: Mutex<Option<Arc<HostBridge>>>,
    identity: Mutex<RuntimeIdentity>,
    catalog: Mutex<Option<VirtualHidProfileCatalog>>,
    handles: Mutex<VirtualHandleTable>,
    last_connect_attempt: Mutex<Option<Instant>>,
    trace_limiter: Mutex<TraceLimiter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuestDriverSnapshot {
    pub identity_state: RuntimeIdentityState,
    pub bridge_connected: bool,
    pub session_trace_id: Option<u32>,
    pub advertised_profiles: usize,
    pub open_handles: usize,
}

impl GuestDriverRuntime {
    pub fn new(config: RuntimeConfig) -> Self {
        let identity = RuntimeIdentity::new(config.allow_debug_fallback());
        let catalog = identity.identity().map(|identity| {
            VirtualHidProfileCatalog::from_identity(identity, config.allow_debug_fallback())
        });
        let trace_limiter = TraceLimiter::new(
            config.trace_reports,
            config.trace_report_limit,
            config.trace_report_max_bytes,
        );

        Self {
            config,
            bridge: Mutex::new(None),
            identity: Mutex::new(identity),
            catalog: Mutex::new(catalog),
            handles: Mutex::new(VirtualHandleTable::default()),
            last_connect_attempt: Mutex::new(None),
            trace_limiter: Mutex::new(trace_limiter),
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn snapshot(&self) -> GuestDriverSnapshot {
        GuestDriverSnapshot {
            identity_state: self
                .identity
                .lock()
                .map(|identity| identity.state())
                .unwrap_or(RuntimeIdentityState::Missing),
            bridge_connected: self.bridge.lock().is_ok_and(|bridge| bridge.is_some()),
            session_trace_id: self.session_trace_id(),
            advertised_profiles: self
                .catalog
                .lock()
                .ok()
                .and_then(|catalog| catalog.as_ref().map(|catalog| catalog.descriptors().len()))
                .unwrap_or(0),
            open_handles: self
                .handles
                .lock()
                .map(|handles| handles.total_open_count())
                .unwrap_or(0),
        }
    }

    pub fn catalog(&self) -> Option<VirtualHidProfileCatalog> {
        self.catalog_result().ok().flatten()
    }

    pub fn session_trace_id(&self) -> Option<u32> {
        self.bridge
            .lock()
            .ok()
            .and_then(|bridge| bridge.as_ref().map(|bridge| bridge.info().session_trace_id))
    }

    pub fn catalog_if_connected(&self) -> Option<VirtualHidProfileCatalog> {
        if self.config.host_bridge_required && !self.bridge_healthy() {
            return None;
        }
        if !self
            .identity
            .lock()
            .is_ok_and(|identity| identity.can_advertise(self.config.host_bridge_required))
        {
            return None;
        }
        self.catalog.lock().ok().and_then(|catalog| catalog.clone())
    }

    pub fn catalog_result(&self) -> Result<Option<VirtualHidProfileCatalog>, GuestDriverError> {
        if self.config.host_bridge_required && !self.ensure_connected_result()? {
            return Ok(None);
        }
        if !self
            .identity
            .lock()
            .is_ok_and(|identity| identity.can_advertise(self.config.host_bridge_required))
        {
            return Ok(None);
        }
        self.catalog
            .lock()
            .map(|catalog| catalog.clone())
            .map_err(|_| GuestDriverError::StatePoisoned("catalog"))
    }

    pub fn can_advertise(&self) -> bool {
        if self.config.host_bridge_required && !self.ensure_connected() {
            return false;
        }
        self.identity
            .lock()
            .is_ok_and(|identity| identity.can_advertise(self.config.host_bridge_required))
    }

    pub fn ensure_connected(&self) -> bool {
        self.ensure_connected_result().unwrap_or(false)
    }

    pub fn ensure_connected_result(&self) -> Result<bool, GuestDriverError> {
        if !self.config.host_bridge_enabled {
            return Ok(false);
        }

        if self.bridge_healthy() {
            return Ok(true);
        }
        self.clear_bridge("stale or missing bridge");

        if !self.should_attempt_connect() {
            return Ok(false);
        }

        self.connect_bridge().map(|()| true)
    }

    pub fn connect_bridge(&self) -> Result<(), GuestDriverError> {
        let bridge = Arc::new(HostBridge::connect(self.config.host_bridge_config())?);
        let identity_payload = bridge.info().identity.clone();
        let catalog = VirtualHidProfileCatalog::from_identity(
            &identity_payload,
            self.config.allow_debug_fallback(),
        );

        *self
            .identity
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("identity"))? = {
            let mut identity = RuntimeIdentity::new(self.config.allow_debug_fallback());
            identity.set_host_identity(identity_payload);
            identity
        };
        *self
            .catalog
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("catalog"))? = Some(catalog);
        *self
            .bridge
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("bridge"))? = Some(bridge);
        Ok(())
    }

    pub fn clear_bridge(&self, _reason: &'static str) {
        let removed_bridge = self.bridge.lock().ok().and_then(|mut bridge| bridge.take());
        drop(removed_bridge);
        self.mark_bridge_stale();
    }

    fn clear_bridge_if_current(&self, current_bridge: &Arc<HostBridge>, _reason: &'static str) {
        let removed_bridge = self.bridge.lock().ok().and_then(|mut bridge| {
            if bridge
                .as_ref()
                .is_some_and(|bridge| Arc::ptr_eq(bridge, current_bridge))
            {
                bridge.take()
            } else {
                None
            }
        });
        if removed_bridge.is_some() {
            drop(removed_bridge);
            self.mark_bridge_stale();
        }
    }

    fn mark_bridge_stale(&self) {
        let next_identity = if let Ok(mut identity) = self.identity.lock() {
            identity.mark_stale();
            identity.identity().cloned()
        } else {
            None
        };
        if let Ok(mut catalog) = self.catalog.lock() {
            *catalog = next_identity.as_ref().map(|identity| {
                VirtualHidProfileCatalog::from_identity(
                    identity,
                    self.config.allow_debug_fallback(),
                )
            });
        }
    }

    pub fn open_profile(
        &self,
        profile: VirtualHidProfile,
    ) -> Result<VirtualHandleId, GuestDriverError> {
        if !self.can_advertise() {
            return Err(GuestDriverError::DeviceNotConnected);
        }
        if !self.profile_available(profile) {
            return Err(GuestDriverError::ProfileUnavailable(profile));
        }
        self.handles
            .lock()
            .map(|mut handles| handles.open(profile))
            .map_err(|_| GuestDriverError::StatePoisoned("handles"))
    }

    pub fn close_handle(&self, handle: VirtualHandleId) -> Result<usize, GuestDriverError> {
        self.handles
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("handles"))?
            .close(handle)
            .ok_or(GuestDriverError::InvalidHandle)
    }

    pub fn handle_profile(
        &self,
        handle: VirtualHandleId,
    ) -> Result<VirtualHidProfile, GuestDriverError> {
        self.handles
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("handles"))?
            .profile(handle)
            .ok_or(GuestDriverError::InvalidHandle)
    }

    pub fn open_count(&self, profile: VirtualHidProfile) -> usize {
        self.handles
            .lock()
            .map(|handles| handles.open_count(profile))
            .unwrap_or(0)
    }

    pub fn is_profile_open(&self, profile: VirtualHidProfile) -> bool {
        self.open_count(profile) > 0
    }

    pub fn copy_next_input_report(
        &self,
        profile: VirtualHidProfile,
        output: &mut [u8],
    ) -> Result<Option<usize>, GuestDriverError> {
        let interface_number = self.connected_interface_number(profile)?;
        self.with_bridge("INPUT", |bridge| {
            bridge.copy_next_input_report(interface_number, output)
        })
    }

    pub fn copy_feature_report(
        &self,
        profile: VirtualHidProfile,
        report_id: u8,
        output: &mut [u8],
    ) -> Result<usize, GuestDriverError> {
        let interface_number = self.connected_interface_number(profile)?;
        let timeout_ms = self.operation_timeout_ms(DEFAULT_GET_FEATURE_TIMEOUT);
        self.with_bridge("GET_FEATURE", |bridge| {
            bridge.copy_feature_report(interface_number, report_id, output, timeout_ms)
        })
    }

    pub fn set_feature(
        &self,
        profile: VirtualHidProfile,
        payload: &[u8],
    ) -> Result<u16, GuestDriverError> {
        let interface_number = self.connected_interface_number(profile)?;
        let timeout_ms = self.operation_timeout_ms(DEFAULT_SET_FEATURE_TIMEOUT);
        self.with_bridge("SET_FEATURE", |bridge| {
            bridge.set_feature(interface_number, payload, timeout_ms)
        })
    }

    pub fn set_output(
        &self,
        profile: VirtualHidProfile,
        payload: &[u8],
    ) -> Result<u16, GuestDriverError> {
        let interface_number = self.connected_interface_number(profile)?;
        let timeout_ms = self.operation_timeout_ms(DEFAULT_FEEDBACK_TIMEOUT);
        self.with_bridge("SET_OUTPUT", |bridge| {
            bridge.set_output(interface_number, payload, timeout_ms)
        })
    }

    pub fn write_report(
        &self,
        profile: VirtualHidProfile,
        payload: &[u8],
    ) -> Result<u16, GuestDriverError> {
        let interface_number = self.connected_interface_number(profile)?;
        let timeout_ms = self.operation_timeout_ms(DEFAULT_FEEDBACK_TIMEOUT);
        self.with_bridge("WRITE", |bridge| {
            bridge.write_report(interface_number, payload, timeout_ms)
        })
    }

    pub fn trace_payload(&self, payload: &[u8]) -> Option<String> {
        let mut limiter = self.trace_limiter.lock().ok()?;
        limiter
            .should_trace()
            .then(|| limiter.render_bytes(payload))
    }

    fn profile_available(&self, profile: VirtualHidProfile) -> bool {
        self.catalog
            .lock()
            .ok()
            .and_then(|catalog| {
                catalog
                    .as_ref()
                    .map(|catalog| catalog.descriptor(profile).is_some())
            })
            .unwrap_or(false)
    }

    fn interface_number(&self, profile: VirtualHidProfile) -> Result<u8, GuestDriverError> {
        self.catalog
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("catalog"))?
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .descriptor(profile)
                    .map(|descriptor| descriptor.interface_number)
            })
            .ok_or(GuestDriverError::ProfileUnavailable(profile))
    }

    fn connected_interface_number(
        &self,
        profile: VirtualHidProfile,
    ) -> Result<u8, GuestDriverError> {
        if !self.ensure_connected() {
            return Err(GuestDriverError::DeviceNotConnected);
        }
        self.interface_number(profile)
    }

    fn with_bridge<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&HostBridge) -> Result<T, HostBridgeError>,
    ) -> Result<T, GuestDriverError> {
        if !self.ensure_connected() {
            return Err(GuestDriverError::DeviceNotConnected);
        }
        let bridge = self
            .bridge
            .lock()
            .map_err(|_| GuestDriverError::StatePoisoned("bridge"))?
            .as_ref()
            .cloned()
            .ok_or(GuestDriverError::DeviceNotConnected)?;
        let result = call(&bridge);
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                if error.should_disconnect_bridge() {
                    self.clear_bridge_if_current(&bridge, operation);
                }
                Err(error.into())
            }
        }
    }

    fn should_attempt_connect(&self) -> bool {
        let now = Instant::now();
        let Ok(mut last_attempt) = self.last_connect_attempt.lock() else {
            return false;
        };
        if last_attempt
            .is_some_and(|last| now.duration_since(last) < self.config.lazy_reconnect_interval)
        {
            return false;
        }
        *last_attempt = Some(now);
        true
    }

    fn operation_timeout_ms(&self, default_timeout: Duration) -> u16 {
        let timeout = if self.config.io_timeout == DEFAULT_IO_TIMEOUT {
            default_timeout
        } else {
            self.config.io_timeout
        };
        timeout.as_millis().min(u128::from(u16::MAX)) as u16
    }

    fn bridge_healthy(&self) -> bool {
        self.bridge.lock().is_ok_and(|bridge| {
            bridge
                .as_ref()
                .is_some_and(|bridge| bridge.input_stats().read_errors == 0)
        })
    }
}

#[derive(Debug)]
pub enum GuestDriverError {
    HostBridge(HostBridgeError),
    DeviceNotConnected,
    InvalidHandle,
    ProfileUnavailable(VirtualHidProfile),
    StatePoisoned(&'static str),
}

impl fmt::Display for GuestDriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HostBridge(error) => write!(f, "{error}"),
            Self::DeviceNotConnected => f.write_str("host bridge is not connected"),
            Self::InvalidHandle => f.write_str("invalid virtual HID handle"),
            Self::ProfileUnavailable(profile) => {
                write!(f, "virtual HID profile is unavailable: {}", profile.label())
            }
            Self::StatePoisoned(name) => write!(f, "guest driver state lock poisoned: {name}"),
        }
    }
}

impl std::error::Error for GuestDriverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HostBridge(error) => Some(error),
            _ => None,
        }
    }
}

impl From<HostBridgeError> for GuestDriverError {
    fn from(value: HostBridgeError) -> Self {
        Self::HostBridge(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest_driver::identity::default_fallback_identity;
    use crate::protocol::{
        CollectionRole, Frame, HelloOk, InputAttach, InputAttachOk, InputReport, MessageType,
        StatusCode, WireDecode, WirePayload, WriteReport, WriteResult,
    };
    use crate::transport::{ChannelStream, TransportAddrs, TransportListeners};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn required_mode_starts_without_advertised_profiles() {
        let runtime = GuestDriverRuntime::new(RuntimeConfig {
            host_bridge_required: true,
            ..RuntimeConfig::default()
        });

        assert!(!runtime.can_advertise());
        assert!(runtime.catalog().is_none());
    }

    #[test]
    fn debug_fallback_can_open_profile_without_bridge() {
        let runtime = GuestDriverRuntime::new(RuntimeConfig {
            replay_enabled: true,
            ..RuntimeConfig::default()
        });

        let handle = runtime.open_profile(VirtualHidProfile::Main).unwrap();

        assert_eq!(
            runtime.handle_profile(handle).unwrap(),
            VirtualHidProfile::Main
        );
        assert!(runtime.is_profile_open(VirtualHidProfile::Main));
        assert_eq!(runtime.open_count(VirtualHidProfile::Main), 1);
        assert_eq!(runtime.close_handle(handle).unwrap(), 0);
        assert!(!runtime.is_profile_open(VirtualHidProfile::Main));
    }

    #[test]
    fn catalog_uses_fallback_identity_in_debug_mode() {
        let runtime = GuestDriverRuntime::new(RuntimeConfig {
            replay_enabled: true,
            ..RuntimeConfig::default()
        });
        let expected = default_fallback_identity();

        assert_eq!(
            runtime.catalog().unwrap().identity().serial,
            expected.serial
        );
    }

    #[test]
    fn stale_bridge_clears_required_catalog() {
        let runtime = GuestDriverRuntime::new(RuntimeConfig::default());
        {
            let mut identity = runtime.identity.lock().unwrap();
            identity.set_host_identity(default_fallback_identity());
        }
        {
            let mut catalog = runtime.catalog.lock().unwrap();
            *catalog = Some(VirtualHidProfileCatalog::from_identity(
                &default_fallback_identity(),
                false,
            ));
        }

        runtime.clear_bridge("test");

        assert!(runtime.catalog().is_none());
        assert!(!runtime.can_advertise());
    }

    #[test]
    fn command_without_catalog_reports_disconnected_before_profile_unavailable() {
        let runtime = GuestDriverRuntime::new(RuntimeConfig {
            host_bridge_required: true,
            ..RuntimeConfig::default()
        });

        let result = runtime.set_feature(VirtualHidProfile::Main, &[0x00]);

        assert!(matches!(result, Err(GuestDriverError::DeviceNotConnected)));
    }

    #[test]
    fn stale_bridge_uses_debug_fallback_catalog_when_allowed() {
        let mut host_identity = default_fallback_identity();
        host_identity.serial = "HOSTSERIAL".to_string();
        let runtime = GuestDriverRuntime::new(RuntimeConfig {
            replay_enabled: true,
            ..RuntimeConfig::default()
        });
        {
            let mut identity = runtime.identity.lock().unwrap();
            identity.set_host_identity(host_identity.clone());
        }
        {
            let mut catalog = runtime.catalog.lock().unwrap();
            *catalog = Some(VirtualHidProfileCatalog::from_identity(
                &host_identity,
                true,
            ));
        }

        runtime.clear_bridge("test");

        assert_eq!(
            runtime.catalog().unwrap().identity().serial,
            default_fallback_identity().serial
        );
        assert!(runtime.can_advertise());
    }

    #[test]
    fn operation_timeouts_follow_low_latency_defaults_unless_overridden() {
        let runtime = GuestDriverRuntime::new(RuntimeConfig::default());

        assert_eq!(
            runtime.operation_timeout_ms(DEFAULT_GET_FEATURE_TIMEOUT),
            100
        );
        assert_eq!(
            runtime.operation_timeout_ms(DEFAULT_SET_FEATURE_TIMEOUT),
            50
        );
        assert_eq!(runtime.operation_timeout_ms(DEFAULT_FEEDBACK_TIMEOUT), 20);

        let runtime = GuestDriverRuntime::new(RuntimeConfig {
            io_timeout: Duration::from_millis(75),
            ..RuntimeConfig::default()
        });

        assert_eq!(
            runtime.operation_timeout_ms(DEFAULT_GET_FEATURE_TIMEOUT),
            75
        );
        assert_eq!(
            runtime.operation_timeout_ms(DEFAULT_SET_FEATURE_TIMEOUT),
            75
        );
        assert_eq!(runtime.operation_timeout_ms(DEFAULT_FEEDBACK_TIMEOUT), 75);
    }

    #[test]
    fn input_pop_is_not_blocked_by_pending_feedback_command() {
        let listeners = TransportListeners::bind(TransportAddrs::loopback(0, 0)).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (write_received_tx, write_received_rx) = mpsc::channel();

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
            write_test_payload(
                &mut control,
                hello.header.id,
                &HelloOk::success(0xCAFE_BABE, 54),
            );
            write_test_payload(&mut control, 0, &default_fallback_identity());

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
            write_test_payload(
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
            let write_payload = WriteReport::decode(&write.payload).unwrap();
            write_received_tx.send(()).unwrap();
            write_test_payload(
                &mut input,
                1,
                &InputReport {
                    interface_number: 2,
                    role: CollectionRole::PuckMain,
                    host_monotonic_us: 42,
                    data: vec![0x79, 0x02],
                },
            );
            thread::sleep(Duration::from_millis(120));
            write_test_payload(
                &mut control,
                write.header.id,
                &WriteResult {
                    status: StatusCode::Ok,
                    bytes_written: write_payload.data.len() as u16,
                    os_error: 0,
                },
            );
        });

        ready_rx.recv().unwrap();
        let runtime = Arc::new(GuestDriverRuntime::new(RuntimeConfig {
            addrs,
            host_bridge_enabled: true,
            host_bridge_required: true,
            io_timeout: Duration::from_millis(250),
            ..RuntimeConfig::default()
        }));
        runtime.connect_bridge().unwrap();

        let writer_runtime = Arc::clone(&runtime);
        let writer = thread::spawn(move || {
            writer_runtime
                .write_report(VirtualHidProfile::Main, &[0x80, 0x00])
                .unwrap()
        });
        write_received_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();

        let mut output = [0_u8; 64];
        let started = Instant::now();
        let count = loop {
            if let Some(count) = runtime
                .copy_next_input_report(VirtualHidProfile::Main, &mut output)
                .unwrap()
            {
                break count;
            }
            assert!(
                started.elapsed() < Duration::from_millis(80),
                "input report was blocked behind pending feedback command"
            );
            thread::sleep(Duration::from_millis(1));
        };

        assert_eq!(count, 2);
        assert_eq!(&output[..2], &[0x79, 0x02]);
        assert_eq!(writer.join().unwrap(), 2);
        server.join().unwrap();
    }

    fn write_test_payload<T: WirePayload>(stream: &mut ChannelStream, id: u32, payload: &T) {
        stream
            .write_frame(&Frame::new(
                T::MESSAGE_TYPE,
                id,
                payload.to_bytes().unwrap(),
            ))
            .unwrap();
    }
}
