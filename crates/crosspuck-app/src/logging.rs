use crosspuck_core::protocol::LogSeverity;
use log::LevelFilter;
use oslog::OsLogger;
use std::env;
use std::ffi::OsString;

const LOG_SUBSYSTEM: &str = "dev.crosspuck.host";
const LOG_LEVEL_ENV: &str = "CROSSPUCK_LOG_LEVEL";
const DEFAULT_LEVEL: LevelFilter = LevelFilter::Info;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoggingConfig {
    pub level: LevelFilter,
    pub source: LogLevelSource,
    pub override_guest_log_level: bool,
    pub invalid_level: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogLevelSource {
    Default,
    Environment,
    Argument,
}

impl LogLevelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Environment => "environment",
            Self::Argument => "argument",
        }
    }
}

pub(crate) fn startup_config() -> LoggingConfig {
    startup_config_from(env::args_os().skip(1), env::var_os(LOG_LEVEL_ENV))
}

pub(crate) fn init(config: &LoggingConfig) -> Result<(), log::SetLoggerError> {
    OsLogger::new(LOG_SUBSYSTEM)
        .level_filter(config.level)
        .init()
}

impl LoggingConfig {
    pub(crate) fn guest_log_level_override(&self) -> Option<LogSeverity> {
        self.override_guest_log_level
            .then(|| log_level_to_severity(self.level))
    }
}

fn startup_config_from(
    args: impl IntoIterator<Item = OsString>,
    env_level: Option<OsString>,
) -> LoggingConfig {
    let mut selected = SelectedLevel {
        raw: env_level,
        source: LogLevelSource::Environment,
    };
    let mut override_guest_log_level = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--override-log-level" {
            override_guest_log_level = true;
            continue;
        }

        if let Some(raw) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix("--log-level="))
        {
            selected = SelectedLevel {
                raw: Some(OsString::from(raw)),
                source: LogLevelSource::Argument,
            };
            continue;
        }

        if arg == "--log-level" {
            selected = SelectedLevel {
                raw: Some(args.next().unwrap_or_else(|| OsString::from("<missing>"))),
                source: LogLevelSource::Argument,
            };
        }
    }

    match selected.raw {
        Some(raw) => match parse_level(&raw) {
            Ok(level) => LoggingConfig {
                level,
                source: selected.source,
                override_guest_log_level,
                invalid_level: None,
            },
            Err(raw) => LoggingConfig {
                level: DEFAULT_LEVEL,
                source: LogLevelSource::Default,
                override_guest_log_level,
                invalid_level: Some(raw),
            },
        },
        None => LoggingConfig {
            level: DEFAULT_LEVEL,
            source: LogLevelSource::Default,
            override_guest_log_level,
            invalid_level: None,
        },
    }
}

fn parse_level(raw: &OsString) -> Result<LevelFilter, String> {
    let Some(raw) = raw.to_str() else {
        return Err("<non-utf8>".to_string());
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => Ok(LevelFilter::Off),
        "error" => Ok(LevelFilter::Error),
        "warn" | "warning" => Ok(LevelFilter::Warn),
        "info" => Ok(LevelFilter::Info),
        "debug" => Ok(LevelFilter::Debug),
        "trace" => Ok(LevelFilter::Trace),
        _ => Err(raw.to_string()),
    }
}

fn level_label(level: LevelFilter) -> &'static str {
    match level {
        LevelFilter::Off => "off",
        LevelFilter::Error => "error",
        LevelFilter::Warn => "warn",
        LevelFilter::Info => "info",
        LevelFilter::Debug => "debug",
        LevelFilter::Trace => "trace",
    }
}

fn log_level_to_severity(level: LevelFilter) -> LogSeverity {
    match level {
        LevelFilter::Off => LogSeverity::Off,
        LevelFilter::Error => LogSeverity::Error,
        LevelFilter::Warn => LogSeverity::Warn,
        LevelFilter::Info => LogSeverity::Info,
        LevelFilter::Debug => LogSeverity::Debug,
        LevelFilter::Trace => LogSeverity::Trace,
    }
}

pub(crate) fn log_startup(config: &LoggingConfig) {
    log::info!(
        "CrossPuck host logging initialized: level={} source={} guest_override={}",
        level_label(config.level),
        config.source.as_str(),
        config
            .guest_log_level_override()
            .map(LogSeverity::as_str)
            .unwrap_or("disabled")
    );

    if let Some(raw_level) = config.invalid_level.as_deref() {
        log::warn!(
            "Invalid log level '{}'; using default level={}",
            raw_level,
            level_label(DEFAULT_LEVEL)
        );
    }
}

#[derive(Debug)]
struct SelectedLevel {
    raw: Option<OsString>,
    source: LogLevelSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn default_level_is_info() {
        let config = startup_config_from([], None);
        assert_eq!(config.level, LevelFilter::Info);
        assert_eq!(config.source, LogLevelSource::Default);
        assert_eq!(config.guest_log_level_override(), None);
        assert_eq!(config.invalid_level, None);
    }

    #[test]
    fn environment_selects_level() {
        let config = startup_config_from([], Some(os("debug")));
        assert_eq!(config.level, LevelFilter::Debug);
        assert_eq!(config.source, LogLevelSource::Environment);
        assert_eq!(config.invalid_level, None);
    }

    #[test]
    fn argument_overrides_environment() {
        let config = startup_config_from([os("--log-level"), os("trace")], Some(os("error")));
        assert_eq!(config.level, LevelFilter::Trace);
        assert_eq!(config.source, LogLevelSource::Argument);
        assert_eq!(config.invalid_level, None);
    }

    #[test]
    fn equals_argument_is_supported() {
        let config = startup_config_from([os("--log-level=warn")], None);
        assert_eq!(config.level, LevelFilter::Warn);
        assert_eq!(config.source, LogLevelSource::Argument);
        assert_eq!(config.invalid_level, None);
    }

    #[test]
    fn invalid_level_falls_back_to_default() {
        let config = startup_config_from([os("--log-level=verbose")], None);
        assert_eq!(config.level, LevelFilter::Info);
        assert_eq!(config.source, LogLevelSource::Default);
        assert_eq!(config.invalid_level.as_deref(), Some("verbose"));
    }

    #[test]
    fn missing_argument_level_falls_back_to_default() {
        let config = startup_config_from([os("--log-level")], Some(os("debug")));
        assert_eq!(config.level, LevelFilter::Info);
        assert_eq!(config.source, LogLevelSource::Default);
        assert_eq!(config.invalid_level.as_deref(), Some("<missing>"));
    }

    #[test]
    fn override_log_level_sends_selected_level_to_guest() {
        let config =
            startup_config_from([os("--override-log-level"), os("--log-level=debug")], None);
        assert_eq!(config.level, LevelFilter::Debug);
        assert_eq!(config.guest_log_level_override(), Some(LogSeverity::Debug));
    }

    #[test]
    fn override_log_level_uses_default_level_without_explicit_level() {
        let config = startup_config_from([os("--override-log-level")], None);
        assert_eq!(config.level, LevelFilter::Info);
        assert_eq!(config.guest_log_level_override(), Some(LogSeverity::Info));
    }
}
