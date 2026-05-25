use crosspuck_core::guest::{GuestError, GuestSession, GuestTransportClient, GuestTransportConfig};
use crosspuck_core::protocol::StatusCode;
use crosspuck_core::transport::TransportAddrs;
use std::fmt;
use std::thread::sleep;
use std::time::{Duration, Instant};

const DEFAULT_CONTROL_PORT: u16 = 28473;
const DEFAULT_INPUT_PORT: u16 = 28474;
const DEFAULT_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_INPUT_POLL_MS: u64 = 100;
const DEFAULT_FEEDBACK_INTERFACE: u8 = 2;
const DEFAULT_PULSE_MS: u64 = 120;
const DEFAULT_COOLDOWN_MS: u64 = 180;
const DEFAULT_RUMBLE_SPEED: u16 = 0x8000;
const RUMBLE_RESEND_MS: u64 = 40;
const TRITON_STATE_REPORT: u8 = 0x42;
const TRITON_BLE_STATE_REPORT: u8 = 0x45;
const PHYSICAL_BUTTON_MASK: u32 = 0x0CCF_FFFF;
const RUMBLE_STOP: [u8; 10] = [0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0];

#[derive(Clone, Debug, Eq, PartialEq)]
struct Config {
    timeout_ms: u64,
    input_poll_ms: u64,
    control_port: u16,
    input_port: u16,
    feedback_interface: u8,
    duration_ms: Option<u64>,
    pulse_ms: u64,
    cooldown_ms: u64,
    low_speed: u16,
    high_speed: u16,
    button_mask: u32,
    trigger: TriggerMode,
    output_api: OutputApi,
    verbose: bool,
}

impl Config {
    fn addrs(&self) -> TransportAddrs {
        TransportAddrs::loopback(self.control_port, self.input_port)
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    fn protocol_timeout_ms(&self) -> u16 {
        self.timeout_ms as u16
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            input_poll_ms: DEFAULT_INPUT_POLL_MS,
            control_port: DEFAULT_CONTROL_PORT,
            input_port: DEFAULT_INPUT_PORT,
            feedback_interface: DEFAULT_FEEDBACK_INTERFACE,
            duration_ms: None,
            pulse_ms: DEFAULT_PULSE_MS,
            cooldown_ms: DEFAULT_COOLDOWN_MS,
            low_speed: DEFAULT_RUMBLE_SPEED,
            high_speed: DEFAULT_RUMBLE_SPEED,
            button_mask: PHYSICAL_BUTTON_MASK,
            trigger: TriggerMode::ButtonEdge,
            output_api: OutputApi::Write,
            verbose: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TriggerMode {
    ButtonEdge,
    AnyReport,
}

impl TriggerMode {
    fn parse(value: &str) -> Result<Self, HapticLoopError> {
        match value {
            "button-edge" | "button" => Ok(Self::ButtonEdge),
            "any-report" | "any" => Ok(Self::AnyReport),
            other => Err(message(format!(
                "invalid --trigger {other}; expected button-edge or any-report"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputApi {
    Write,
    SetOutput,
}

impl OutputApi {
    fn parse(value: &str) -> Result<Self, HapticLoopError> {
        match value {
            "write" => Ok(Self::Write),
            "set-output" | "set_output" => Ok(Self::SetOutput),
            other => Err(message(format!(
                "invalid --output-api {other}; expected write or set-output"
            ))),
        }
    }
}

#[derive(Debug)]
pub(crate) enum HapticLoopError {
    Message(String),
    Guest(GuestError),
}

impl fmt::Display for HapticLoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Guest(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HapticLoopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Guest(error) => Some(error),
        }
    }
}

impl From<GuestError> for HapticLoopError {
    fn from(value: GuestError) -> Self {
        Self::Guest(value)
    }
}

pub(crate) fn run(args: impl IntoIterator<Item = String>) -> Result<(), HapticLoopError> {
    let config = parse_args(args)?;
    let addrs = config.addrs();
    println!(
        "CONNECT control={} input={} timeout={}ms trigger={:?} output_api={:?}",
        addrs.control, addrs.input, config.timeout_ms, config.trigger, config.output_api
    );

    let mut session = GuestTransportClient::connect(GuestTransportConfig {
        addrs,
        connect_timeout: config.timeout(),
        io_timeout: config.timeout(),
        guest_label: "crosspuck-cli-haptic-loop".to_string(),
        ..GuestTransportConfig::default()
    })?;

    println!(
        "CONNECTED session_id={} serial={}",
        session.session_id(),
        session.identity().serial
    );
    println!(
        "HAPTIC_LOOP feedback_interface={} pulse_ms={} cooldown_ms={} low=0x{:04X} high=0x{:04X}",
        config.feedback_interface,
        config.pulse_ms,
        config.cooldown_ms,
        config.low_speed,
        config.high_speed
    );
    println!("Press a controller button to trigger a short rumble. Stop with Ctrl+C.");

    run_loop(&mut session, &config)
}

fn run_loop(session: &mut GuestSession, config: &Config) -> Result<(), HapticLoopError> {
    session.set_input_read_timeout(Some(Duration::from_millis(config.input_poll_ms)))?;

    let started_at = Instant::now();
    let deadline = config
        .duration_ms
        .map(|duration_ms| started_at + Duration::from_millis(duration_ms));
    let mut detector = ButtonDetector::new(config.button_mask, config.trigger);
    let mut cooldown_until = started_at;
    let mut reports = 0_u64;
    let mut triggers = 0_u64;
    let mut timeouts = 0_u64;

    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }

        match session.read_input_report() {
            Ok(report) => {
                reports += 1;
                let event = detector.observe(&report.data);
                if config.verbose || event.triggered {
                    println!(
                        "INPUT seq={} interface={} role={:?} len={} buttons={} trigger={} head={}",
                        report.sequence,
                        report.interface_number,
                        report.role,
                        report.data.len(),
                        event
                            .buttons
                            .map(|buttons| format!("0x{buttons:08X}"))
                            .unwrap_or_else(|| "-".to_string()),
                        event.triggered,
                        hex_head(&report.data, 16)
                    );
                }
                if event.triggered && Instant::now() >= cooldown_until {
                    triggers += 1;
                    play_rumble_pulse(session, config)?;
                    cooldown_until = Instant::now() + Duration::from_millis(config.cooldown_ms);
                }
            }
            Err(error) if error.is_timeout_or_would_block() => {
                timeouts += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }

    send_feedback(session, config, &RUMBLE_STOP)?;
    println!(
        "HAPTIC_LOOP_SUMMARY elapsed_ms={} reports={} triggers={} timeouts={}",
        started_at.elapsed().as_millis(),
        reports,
        triggers,
        timeouts
    );
    Ok(())
}

fn play_rumble_pulse(session: &mut GuestSession, config: &Config) -> Result<(), HapticLoopError> {
    let packet = rumble_packet(config.low_speed, config.high_speed);
    let started_at = Instant::now();
    let pulse_for = Duration::from_millis(config.pulse_ms);

    println!("RUMBLE start packet={}", hex_head(&packet, packet.len()));
    while started_at.elapsed() < pulse_for {
        send_feedback(session, config, &packet)?;
        sleep(Duration::from_millis(RUMBLE_RESEND_MS));
    }
    send_feedback(session, config, &RUMBLE_STOP)?;
    println!("RUMBLE stop");
    Ok(())
}

fn send_feedback(
    session: &mut GuestSession,
    config: &Config,
    packet: &[u8],
) -> Result<(), HapticLoopError> {
    match config.output_api {
        OutputApi::Write => {
            let result = session.write_report(
                config.feedback_interface,
                config.protocol_timeout_ms(),
                packet,
            )?;
            ensure_ok("WRITE", result.status)?;
        }
        OutputApi::SetOutput => {
            let result = session.set_output(
                config.feedback_interface,
                config.protocol_timeout_ms(),
                packet,
            )?;
            ensure_ok("SET_OUTPUT", result.status)?;
        }
    }
    Ok(())
}

fn ensure_ok(operation: &str, status: StatusCode) -> Result<(), HapticLoopError> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(message(format!("{operation} failed with status={status}")))
    }
}

fn rumble_packet(low_speed: u16, high_speed: u16) -> [u8; 10] {
    let low = low_speed.to_le_bytes();
    let high = high_speed.to_le_bytes();
    [0x80, 0, 0, 0, low[0], low[1], 0, high[0], high[1], 0]
}

#[derive(Debug)]
struct ButtonDetector {
    button_mask: u32,
    trigger: TriggerMode,
    last_buttons: Option<u32>,
    primed_any_report: bool,
}

impl ButtonDetector {
    fn new(button_mask: u32, trigger: TriggerMode) -> Self {
        Self {
            button_mask,
            trigger,
            last_buttons: None,
            primed_any_report: false,
        }
    }

    fn observe(&mut self, data: &[u8]) -> DetectionEvent {
        let buttons = triton_buttons(data);
        let triggered = match self.trigger {
            TriggerMode::ButtonEdge => match buttons {
                Some(buttons) => {
                    let masked = buttons & self.button_mask;
                    let previous = self.last_buttons.replace(masked);
                    previous.is_some_and(|previous| masked & !previous != 0)
                }
                None => false,
            },
            TriggerMode::AnyReport => {
                let was_primed = self.primed_any_report;
                self.primed_any_report = true;
                was_primed
            }
        };

        DetectionEvent { buttons, triggered }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DetectionEvent {
    buttons: Option<u32>,
    triggered: bool,
}

fn triton_buttons(data: &[u8]) -> Option<u32> {
    if data.len() < 6 || !matches!(data[0], TRITON_STATE_REPORT | TRITON_BLE_STATE_REPORT) {
        return None;
    }
    Some(u32::from_le_bytes([data[2], data[3], data[4], data[5]]))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, HapticLoopError> {
    let mut config = Config::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--timeout-ms" => {
                let timeout_ms = parse_next::<u64>(&mut args, "--timeout-ms")?;
                if timeout_ms == 0 || timeout_ms > u16::MAX as u64 {
                    return Err(message("--timeout-ms must be in 1..=65535"));
                }
                config.timeout_ms = timeout_ms;
            }
            "--input-poll-ms" => {
                let input_poll_ms = parse_next::<u64>(&mut args, "--input-poll-ms")?;
                if input_poll_ms == 0 {
                    return Err(message("--input-poll-ms must be greater than 0"));
                }
                config.input_poll_ms = input_poll_ms;
            }
            "--control-port" => config.control_port = parse_next(&mut args, "--control-port")?,
            "--input-port" => config.input_port = parse_next(&mut args, "--input-port")?,
            "--interface" => {
                config.feedback_interface = parse_next(&mut args, "--interface")?;
            }
            "--duration-ms" => {
                let duration_ms = parse_next::<u64>(&mut args, "--duration-ms")?;
                if duration_ms == 0 {
                    return Err(message("--duration-ms must be greater than 0"));
                }
                config.duration_ms = Some(duration_ms);
            }
            "--pulse-ms" => {
                let pulse_ms = parse_next::<u64>(&mut args, "--pulse-ms")?;
                if pulse_ms == 0 {
                    return Err(message("--pulse-ms must be greater than 0"));
                }
                config.pulse_ms = pulse_ms;
            }
            "--cooldown-ms" => {
                let cooldown_ms = parse_next::<u64>(&mut args, "--cooldown-ms")?;
                if cooldown_ms == 0 {
                    return Err(message("--cooldown-ms must be greater than 0"));
                }
                config.cooldown_ms = cooldown_ms;
            }
            "--low" => config.low_speed = parse_u16(&next_arg(&mut args, "--low")?)?,
            "--high" => config.high_speed = parse_u16(&next_arg(&mut args, "--high")?)?,
            "--button-mask" => {
                config.button_mask = parse_u32(&next_arg(&mut args, "--button-mask")?)?;
            }
            "--trigger" => {
                config.trigger = TriggerMode::parse(&next_arg(&mut args, "--trigger")?)?;
            }
            "--output-api" => {
                config.output_api = OutputApi::parse(&next_arg(&mut args, "--output-api")?)?;
            }
            "--verbose" => config.verbose = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            unknown => {
                return Err(message(format!(
                    "unknown haptic-loop argument: {unknown}\n\nUse `haptic-loop --help`."
                )));
            }
        }
    }

    Ok(config)
}

pub(crate) fn print_help() {
    println!(
        r#"crosspuck-host haptic-loop

Runs a local guest-driver feedback loop using crosspuck_core::guest. It receives
host input reports through the input channel and sends a short Triton rumble
output report back through the control channel when a controller button is
pressed.

Usage:
  cargo run -p crosspuck-cli -- haptic-loop [options]
  crosspuck-host haptic-loop [options]

Options:
  --control-port <port>       control channel port (default: 28473)
  --input-port <port>         input channel port (default: 28474)
  --timeout-ms <ms>           connect/control timeout (default: 2000)
  --input-poll-ms <ms>        input read poll timeout (default: 100)
  --interface <n>             feedback target HID interface (default: 2)
  --duration-ms <ms>          stop automatically after this duration
  --pulse-ms <ms>             rumble pulse duration (default: 120)
  --cooldown-ms <ms>          minimum gap between rumble pulses (default: 180)
  --low <hex|dec>             low-frequency rumble speed (default: 0x8000)
  --high <hex|dec>            high-frequency rumble speed (default: 0x8000)
  --button-mask <hex|dec>     Triton physical button mask (default: 0x0CCFFFFF)
  --trigger <mode>            button-edge or any-report (default: button-edge)
  --output-api <api>          write or set-output (default: write)
  --verbose                   log every input report
  -h, --help                  print this help

Examples:
  cargo run -p crosspuck-cli -- haptic-loop
  cargo run -p crosspuck-cli -- haptic-loop --duration-ms 30000
  cargo run -p crosspuck-cli -- haptic-loop --pulse-ms 80 --low 0xffff --high 0xffff
  cargo run -p crosspuck-cli -- haptic-loop --trigger any-report --duration-ms 5000
"#
    );
}

fn parse_next<T>(args: &mut impl Iterator<Item = String>, label: &str) -> Result<T, HapticLoopError>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    let value = next_arg(args, label)?;
    value
        .parse::<T>()
        .map_err(|error| message(format!("invalid {label}: {error}")))
}

fn next_arg(
    args: &mut impl Iterator<Item = String>,
    label: &str,
) -> Result<String, HapticLoopError> {
    args.next()
        .ok_or_else(|| message(format!("missing value for {label}")))
}

fn parse_u16(value: &str) -> Result<u16, HapticLoopError> {
    parse_prefixed_u64(value).and_then(|value| {
        u16::try_from(value).map_err(|_| message(format!("value out of u16 range: {value}")))
    })
}

fn parse_u32(value: &str) -> Result<u32, HapticLoopError> {
    parse_prefixed_u64(value).and_then(|value| {
        u32::try_from(value).map_err(|_| message(format!("value out of u32 range: {value}")))
    })
}

fn parse_prefixed_u64(value: &str) -> Result<u64, HapticLoopError> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
            .map_err(|error| message(format!("invalid hex {value}: {error}")))
    } else {
        value
            .parse::<u64>()
            .map_err(|error| message(format!("invalid integer {value}: {error}")))
    }
}

fn hex_head(bytes: &[u8], limit: usize) -> String {
    let mut out = bytes
        .iter()
        .take(limit)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > limit {
        out.push_str(" ...");
    }
    out
}

fn message(message: impl Into<String>) -> HapticLoopError {
    HapticLoopError::Message(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_triton_buttons_from_usb_and_ble_reports() {
        assert_eq!(triton_buttons(&[0x42, 0x10, 0x01, 0, 0, 0]), Some(1));
        assert_eq!(
            triton_buttons(&[0x45, 0xE4, 0, 0, 0, 0x10]),
            Some(0x1000_0000)
        );
        assert_eq!(triton_buttons(&[0x79, 0x02]), None);
    }

    #[test]
    fn button_detector_triggers_on_rising_edge_only() {
        let mut detector = ButtonDetector::new(PHYSICAL_BUTTON_MASK, TriggerMode::ButtonEdge);
        assert!(!detector.observe(&[0x42, 0, 0, 0, 0, 0]).triggered);
        assert!(detector.observe(&[0x42, 1, 0x01, 0, 0, 0]).triggered);
        assert!(!detector.observe(&[0x42, 2, 0x01, 0, 0, 0]).triggered);
        assert!(!detector.observe(&[0x42, 3, 0, 0, 0, 0]).triggered);
        assert!(detector.observe(&[0x42, 4, 0x02, 0, 0, 0]).triggered);
    }

    #[test]
    fn button_detector_ignores_touch_only_bits_by_default() {
        let mut detector = ButtonDetector::new(PHYSICAL_BUTTON_MASK, TriggerMode::ButtonEdge);
        assert!(!detector.observe(&[0x42, 0, 0, 0, 0, 0]).triggered);
        assert!(!detector.observe(&[0x42, 1, 0, 0, 0, 0x10]).triggered);
    }

    #[test]
    fn builds_rumble_packet() {
        assert_eq!(
            rumble_packet(0x1234, 0x5678),
            [0x80, 0, 0, 0, 0x34, 0x12, 0, 0x78, 0x56, 0]
        );
    }

    #[test]
    fn parse_accepts_core_options() {
        let config = parse_args(strings(&[
            "--control-port",
            "30001",
            "--input-port",
            "30002",
            "--duration-ms",
            "5000",
            "--pulse-ms",
            "80",
            "--cooldown-ms",
            "120",
            "--low",
            "0xffff",
            "--high",
            "32768",
            "--button-mask",
            "0x00000003",
            "--trigger",
            "any-report",
            "--output-api",
            "set-output",
            "--verbose",
        ]))
        .unwrap();

        assert_eq!(config.control_port, 30001);
        assert_eq!(config.input_port, 30002);
        assert_eq!(config.duration_ms, Some(5000));
        assert_eq!(config.pulse_ms, 80);
        assert_eq!(config.cooldown_ms, 120);
        assert_eq!(config.low_speed, 0xffff);
        assert_eq!(config.high_speed, 32768);
        assert_eq!(config.button_mask, 0x0000_0003);
        assert_eq!(config.trigger, TriggerMode::AnyReport);
        assert_eq!(config.output_api, OutputApi::SetOutput);
        assert!(config.verbose);
    }
}
