use crate::transport::TransportAddrs;
use std::env;
use std::time::Duration;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(1_000);
pub const DRIVER_MAX_CONNECT_TIMEOUT: Duration = Duration::from_millis(1_000);
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(2_000);
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(50);
pub const DEFAULT_LAZY_RECONNECT_INTERVAL: Duration = Duration::from_millis(1_000);
pub const DEFAULT_INPUT_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_TRACE_REPORT_LIMIT: usize = 2048;
pub const DEFAULT_TRACE_REPORT_MAX_BYTES: usize = 128;

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
    pub guest_label: String,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|name| env::var(name).ok())
    }

    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        Self::from_lookup_with_defaults(&mut lookup, false, false, false)
    }

    pub fn driver_from_env() -> Self {
        Self::driver_from_lookup(|name| env::var(name).ok())
    }

    pub fn driver_from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let mut config = Self::from_lookup_with_defaults(&mut lookup, true, true, true);
        config.connect_timeout = config.connect_timeout.min(DRIVER_MAX_CONNECT_TIMEOUT);
        config
    }

    fn from_lookup_with_defaults(
        lookup: &mut impl FnMut(&str) -> Option<String>,
        host_bridge_enabled_default: bool,
        host_bridge_required_default: bool,
        trace_reports_default: bool,
    ) -> Self {
        Self {
            addrs: TransportAddrs::default(),
            host_bridge_enabled: env_bool(
                lookup,
                "CROSSPUCK_HOST_BRIDGE",
                host_bridge_enabled_default,
            ),
            host_bridge_required: env_bool(
                lookup,
                "CROSSPUCK_HOST_BRIDGE_REQUIRED",
                host_bridge_required_default,
            ),
            replay_enabled: env_bool(lookup, "CROSSPUCK_REPLAY_ENABLED", false),
            trace_reports: env_bool(lookup, "CROSSPUCK_TRACE_REPORTS", trace_reports_default),
            connect_timeout: env_duration_ms(
                lookup,
                "CROSSPUCK_HOST_BRIDGE_CONNECT_TIMEOUT_MS",
                DEFAULT_CONNECT_TIMEOUT,
            ),
            handshake_timeout: env_duration_ms(
                lookup,
                "CROSSPUCK_HOST_BRIDGE_HANDSHAKE_TIMEOUT_MS",
                DEFAULT_HANDSHAKE_TIMEOUT,
            ),
            io_timeout: env_duration_ms(
                lookup,
                "CROSSPUCK_HOST_BRIDGE_IO_TIMEOUT_MS",
                DEFAULT_IO_TIMEOUT,
            ),
            lazy_reconnect_interval: env_duration_ms(
                lookup,
                "CROSSPUCK_HOST_BRIDGE_RECONNECT_INTERVAL_MS",
                DEFAULT_LAZY_RECONNECT_INTERVAL,
            ),
            input_queue_capacity: env_usize(
                lookup,
                "CROSSPUCK_INPUT_QUEUE_CAPACITY",
                DEFAULT_INPUT_QUEUE_CAPACITY,
            )
            .max(1),
            trace_report_limit: env_usize(
                lookup,
                "CROSSPUCK_TRACE_REPORT_LIMIT",
                DEFAULT_TRACE_REPORT_LIMIT,
            ),
            trace_report_max_bytes: env_usize(
                lookup,
                "CROSSPUCK_TRACE_REPORT_MAX_BYTES",
                DEFAULT_TRACE_REPORT_MAX_BYTES,
            ),
            guest_label: lookup("CROSSPUCK_GUEST_LABEL")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "crosspuck-driver".to_string()),
        }
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
            guest_label: "crosspuck-driver".to_string(),
        }
    }
}

fn env_bool(lookup: &mut impl FnMut(&str) -> Option<String>, name: &str, default: bool) -> bool {
    let Some(value) = lookup(name) else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

fn env_duration_ms(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    name: &str,
    default: Duration,
) -> Duration {
    lookup(name)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn env_usize(lookup: &mut impl FnMut(&str) -> Option<String>, name: &str, default: usize) -> usize {
    lookup(name)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn defaults_disable_host_bridge_and_replay() {
        let config = RuntimeConfig::from_lookup(|_| None);

        assert!(!config.host_bridge_enabled);
        assert!(!config.host_bridge_required);
        assert!(!config.replay_enabled);
        assert!(!config.allow_debug_fallback());
        assert_eq!(config.handshake_timeout, DEFAULT_HANDSHAKE_TIMEOUT);
        assert_eq!(config.io_timeout, DEFAULT_IO_TIMEOUT);
    }

    #[test]
    fn driver_defaults_enable_required_host_bridge() {
        let config = RuntimeConfig::driver_from_lookup(|_| None);

        assert!(config.host_bridge_enabled);
        assert!(config.host_bridge_required);
        assert!(!config.replay_enabled);
        assert!(config.trace_reports);
        assert!(!config.allow_debug_fallback());
        assert_eq!(config.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(config.handshake_timeout, DEFAULT_HANDSHAKE_TIMEOUT);
        assert_eq!(config.io_timeout, DEFAULT_IO_TIMEOUT);
        assert_eq!(
            config.lazy_reconnect_interval,
            DEFAULT_LAZY_RECONNECT_INTERVAL
        );
    }

    #[test]
    fn driver_defaults_can_be_overridden_by_env() {
        let values = HashMap::from([
            ("CROSSPUCK_HOST_BRIDGE", "0"),
            ("CROSSPUCK_HOST_BRIDGE_REQUIRED", "0"),
            ("CROSSPUCK_REPLAY_ENABLED", "1"),
            ("CROSSPUCK_TRACE_REPORTS", "1"),
        ]);
        let config = RuntimeConfig::driver_from_lookup(|name| {
            values.get(name).map(|value| value.to_string())
        });

        assert!(!config.host_bridge_enabled);
        assert!(!config.host_bridge_required);
        assert!(config.replay_enabled);
        assert!(config.trace_reports);
        assert!(config.allow_debug_fallback());
    }

    #[test]
    fn driver_connect_timeout_is_capped_for_steam_startup() {
        let values = HashMap::from([("CROSSPUCK_HOST_BRIDGE_CONNECT_TIMEOUT_MS", "5000")]);
        let config = RuntimeConfig::driver_from_lookup(|name| {
            values.get(name).map(|value| value.to_string())
        });

        assert_eq!(config.connect_timeout, DRIVER_MAX_CONNECT_TIMEOUT);
    }

    #[test]
    fn parses_required_bridge_and_timeouts() {
        let values = HashMap::from([
            ("CROSSPUCK_HOST_BRIDGE", "1"),
            ("CROSSPUCK_HOST_BRIDGE_REQUIRED", "true"),
            ("CROSSPUCK_REPLAY_ENABLED", "0"),
            ("CROSSPUCK_HOST_BRIDGE_CONNECT_TIMEOUT_MS", "2500"),
            ("CROSSPUCK_HOST_BRIDGE_HANDSHAKE_TIMEOUT_MS", "1500"),
            ("CROSSPUCK_HOST_BRIDGE_IO_TIMEOUT_MS", "75"),
            ("CROSSPUCK_INPUT_QUEUE_CAPACITY", "8"),
        ]);
        let config =
            RuntimeConfig::from_lookup(|name| values.get(name).map(|value| value.to_string()));

        assert!(config.host_bridge_enabled);
        assert!(config.host_bridge_required);
        assert!(!config.replay_enabled);
        assert_eq!(config.connect_timeout, Duration::from_millis(2_500));
        assert_eq!(config.handshake_timeout, Duration::from_millis(1_500));
        assert_eq!(config.io_timeout, Duration::from_millis(75));
        assert_eq!(config.input_queue_capacity, 8);
    }
}
