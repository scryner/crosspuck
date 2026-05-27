use crate::protocol::LogSeverity;
use crate::transport::TransportAddrs;
use std::time::Duration;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(1_000);
pub const DRIVER_MAX_CONNECT_TIMEOUT: Duration = Duration::from_millis(1_000);
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(2_000);
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(1_000);
pub const DEFAULT_LAZY_RECONNECT_INTERVAL: Duration = Duration::from_millis(1_000);
pub const DEFAULT_INPUT_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_TRACE_REPORT_LIMIT: usize = 2048;
pub const DEFAULT_TRACE_REPORT_MAX_BYTES: usize = 128;
pub const DEFAULT_LOG_LEVEL: GuestLogLevel = GuestLogLevel::Info;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum GuestLogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl GuestLogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    pub fn allows(self, level: Self) -> bool {
        level != Self::Off && self >= level
    }
}

impl From<LogSeverity> for GuestLogLevel {
    fn from(value: LogSeverity) -> Self {
        match value {
            LogSeverity::Off => Self::Off,
            LogSeverity::Error => Self::Error,
            LogSeverity::Warn => Self::Warn,
            LogSeverity::Info => Self::Info,
            LogSeverity::Debug => Self::Debug,
            LogSeverity::Trace => Self::Trace,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    pub addrs: TransportAddrs,
    pub host_bridge_enabled: bool,
    pub host_bridge_required: bool,
    pub replay_enabled: bool,
    pub trace_reports: bool,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
    pub io_timeout: Duration,
    pub lazy_reconnect_interval: Duration,
    pub input_queue_capacity: usize,
    pub trace_report_limit: usize,
    pub trace_report_max_bytes: usize,
    pub log_level: GuestLogLevel,
    pub guest_label: String,
}

impl RuntimeConfig {
    pub fn driver_defaults() -> Self {
        let mut config = Self {
            host_bridge_enabled: true,
            host_bridge_required: true,
            ..Self::default()
        };
        config.connect_timeout = config.connect_timeout.min(DRIVER_MAX_CONNECT_TIMEOUT);
        config
    }

    pub fn allow_debug_fallback(&self) -> bool {
        !self.host_bridge_required && self.replay_enabled
    }

    pub fn host_bridge_config(&self) -> super::bridge::HostBridgeConfig {
        super::bridge::HostBridgeConfig {
            addrs: self.addrs,
            connect_timeout: self.connect_timeout,
            handshake_timeout: self.handshake_timeout,
            io_timeout: self.io_timeout,
            guest_label: self.guest_label.clone(),
            input_queue_capacity: self.input_queue_capacity,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            addrs: TransportAddrs::default(),
            host_bridge_enabled: false,
            host_bridge_required: false,
            replay_enabled: false,
            trace_reports: false,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            io_timeout: DEFAULT_IO_TIMEOUT,
            lazy_reconnect_interval: DEFAULT_LAZY_RECONNECT_INTERVAL,
            input_queue_capacity: DEFAULT_INPUT_QUEUE_CAPACITY,
            trace_report_limit: DEFAULT_TRACE_REPORT_LIMIT,
            trace_report_max_bytes: DEFAULT_TRACE_REPORT_MAX_BYTES,
            log_level: DEFAULT_LOG_LEVEL,
            guest_label: "crosspuck-driver".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_disable_host_bridge_and_replay() {
        let config = RuntimeConfig::default();

        assert!(!config.host_bridge_enabled);
        assert!(!config.host_bridge_required);
        assert!(!config.replay_enabled);
        assert!(!config.allow_debug_fallback());
        assert_eq!(config.handshake_timeout, DEFAULT_HANDSHAKE_TIMEOUT);
        assert_eq!(config.io_timeout, DEFAULT_IO_TIMEOUT);
    }

    #[test]
    fn driver_defaults_enable_required_host_bridge() {
        let config = RuntimeConfig::driver_defaults();

        assert!(config.host_bridge_enabled);
        assert!(config.host_bridge_required);
        assert!(!config.replay_enabled);
        assert!(!config.trace_reports);
        assert_eq!(config.log_level, GuestLogLevel::Info);
        assert!(!config.allow_debug_fallback());
        assert_eq!(config.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert!(config.connect_timeout <= DRIVER_MAX_CONNECT_TIMEOUT);
        assert_eq!(config.handshake_timeout, DEFAULT_HANDSHAKE_TIMEOUT);
        assert_eq!(config.io_timeout, DEFAULT_IO_TIMEOUT);
        assert_eq!(
            config.lazy_reconnect_interval,
            DEFAULT_LAZY_RECONNECT_INTERVAL
        );
    }
}
