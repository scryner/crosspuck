use super::bridge::{HostBridge, HostBridgeError};
use super::config::RuntimeConfig;
use super::handles::{VirtualHandleId, VirtualHandleTable};
use super::identity::{RuntimeIdentity, RuntimeIdentityState};
use super::profile::{VirtualHidProfile, VirtualHidProfileCatalog};
use super::trace::TraceLimiter;
use std::fmt;
use std::sync::Mutex;
use std::time::Instant;

pub struct GuestDriverRuntime {
    config: RuntimeConfig,
    bridge: Mutex<Option<HostBridge>>,
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
        if self.config.host_bridge_required && !self.ensure_connected() {
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

    pub fn can_advertise(&self) -> bool {
        if self.config.host_bridge_required && !self.ensure_connected() {
            return false;
        }
        self.identity
            .lock()
            .is_ok_and(|identity| identity.can_advertise(self.config.host_bridge_required))
    }

    pub fn ensure_connected(&self) -> bool {
        if !self.config.host_bridge_enabled {
            return false;
        }

        if self.bridge.lock().is_ok_and(|bridge| {
            bridge
                .as_ref()
                .is_some_and(|bridge| bridge.input_stats().read_errors == 0)
        }) {
            return true;
        }
        self.clear_bridge("stale or missing bridge");

        if !self.should_attempt_connect() {
            return false;
        }

        self.connect_bridge().is_ok()
    }

    pub fn connect_bridge(&self) -> Result<(), GuestDriverError> {
        let bridge = HostBridge::connect(self.config.host_bridge_config())?;
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
        if let Ok(mut bridge) = self.bridge.lock() {
            bridge.take();
        }
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
        let interface_number = self.interface_number(profile)?;
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
        let interface_number = self.interface_number(profile)?;
        let timeout_ms = self.command_timeout_ms();
        self.with_bridge("GET_FEATURE", |bridge| {
            bridge.copy_feature_report(interface_number, report_id, output, timeout_ms)
        })
    }

    pub fn set_feature(
        &self,
        profile: VirtualHidProfile,
        payload: &[u8],
    ) -> Result<u16, GuestDriverError> {
        let interface_number = self.interface_number(profile)?;
        let timeout_ms = self.command_timeout_ms();
        self.with_bridge("SET_FEATURE", |bridge| {
            bridge.set_feature(interface_number, payload, timeout_ms)
        })
    }

    pub fn set_output(
        &self,
        profile: VirtualHidProfile,
        payload: &[u8],
    ) -> Result<u16, GuestDriverError> {
        let interface_number = self.interface_number(profile)?;
        let timeout_ms = self.command_timeout_ms();
        self.with_bridge("SET_OUTPUT", |bridge| {
            bridge.set_output(interface_number, payload, timeout_ms)
        })
    }

    pub fn write_report(
        &self,
        profile: VirtualHidProfile,
        payload: &[u8],
    ) -> Result<u16, GuestDriverError> {
        let interface_number = self.interface_number(profile)?;
        let timeout_ms = self.command_timeout_ms();
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

    fn with_bridge<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&HostBridge) -> Result<T, HostBridgeError>,
    ) -> Result<T, GuestDriverError> {
        if !self.ensure_connected() {
            return Err(GuestDriverError::DeviceNotConnected);
        }
        let result = {
            let bridge = self
                .bridge
                .lock()
                .map_err(|_| GuestDriverError::StatePoisoned("bridge"))?;
            let bridge = bridge
                .as_ref()
                .ok_or(GuestDriverError::DeviceNotConnected)?;
            call(bridge)
        };
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                if error.should_disconnect_bridge() {
                    self.clear_bridge(operation);
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

    fn command_timeout_ms(&self) -> u16 {
        self.config.io_timeout.as_millis().min(u128::from(u16::MAX)) as u16
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
}
