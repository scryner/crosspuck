use crosspuck_core::guest::{GuestError, GuestSession, GuestTransportClient, GuestTransportConfig};
use crosspuck_core::protocol::StatusCode;
use crosspuck_core::transport::TransportAddrs;
use std::fmt;
use std::time::Duration;

const DEFAULT_CONTROL_PORT: u16 = 28473;
const DEFAULT_INPUT_PORT: u16 = 28474;
const DEFAULT_TIMEOUT_MS: u64 = 2_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Config {
    timeout_ms: u64,
    control_port: u16,
    input_port: u16,
    connect_count: usize,
    allow_input_timeout: bool,
    operations: Vec<Operation>,
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
            control_port: DEFAULT_CONTROL_PORT,
            input_port: DEFAULT_INPUT_PORT,
            connect_count: 1,
            allow_input_timeout: false,
            operations: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Operation {
    Reports(usize),
    GetFeature {
        interface_number: u8,
        report_id: u8,
        len: u16,
    },
    Write {
        interface_number: u8,
        payload: Vec<u8>,
    },
    SetFeature {
        interface_number: u8,
        payload: Vec<u8>,
    },
    SetOutput {
        interface_number: u8,
        payload: Vec<u8>,
    },
}

#[derive(Debug)]
pub(crate) enum GuestDriverError {
    Message(String),
    Guest(GuestError),
}

impl fmt::Display for GuestDriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Guest(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for GuestDriverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Guest(error) => Some(error),
        }
    }
}

impl From<GuestError> for GuestDriverError {
    fn from(value: GuestError) -> Self {
        Self::Guest(value)
    }
}

pub(crate) fn run(args: impl IntoIterator<Item = String>) -> Result<(), GuestDriverError> {
    let config = parse_args(args)?;

    for attempt in 1..=config.connect_count {
        run_attempt(&config, attempt)?;
    }

    Ok(())
}

fn run_attempt(config: &Config, attempt: usize) -> Result<(), GuestDriverError> {
    let addrs = config.addrs();
    println!(
        "CONNECT attempt={attempt}/{} control={} input={} timeout={}ms",
        config.connect_count, addrs.control, addrs.input, config.timeout_ms
    );

    let mut session = GuestTransportClient::connect(GuestTransportConfig {
        addrs,
        connect_timeout: config.timeout(),
        io_timeout: config.timeout(),
        guest_label: "crosspuck-cli-guest-driver".to_string(),
        ..GuestTransportConfig::default()
    })?;

    println!(
        "CONNECTED session_id={} label={}",
        session.session_id(),
        session.guest_label()
    );
    print_identity(&session);

    for operation in &config.operations {
        run_operation(&mut session, config, operation)?;
    }

    println!("DISCONNECT attempt={attempt}/{}", config.connect_count);
    Ok(())
}

fn run_operation(
    session: &mut GuestSession,
    config: &Config,
    operation: &Operation,
) -> Result<(), GuestDriverError> {
    match operation {
        Operation::Reports(count) => {
            for index in 0..*count {
                match session.read_input_report() {
                    Ok(report) => {
                        println!(
                            "INPUT seq={} interface={} role={:?} len={} head={}",
                            report.sequence,
                            report.interface_number,
                            report.role,
                            report.data.len(),
                            hex_head(&report.data, 16)
                        );
                    }
                    Err(error)
                        if config.allow_input_timeout && error.is_timeout_or_would_block() =>
                    {
                        println!(
                            "INPUT timeout allowed index={}/{} timeout={}ms",
                            index + 1,
                            count,
                            config.timeout_ms
                        );
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Operation::GetFeature {
            interface_number,
            report_id,
            len,
        } => {
            let result = session.get_feature(
                *interface_number,
                *report_id,
                *len,
                config.protocol_timeout_ms(),
            )?;
            print_status("GET_FEATURE", result.status);
            println!(
                "FEATURE os_error={} len={} head={}",
                result.os_error,
                result.data.len(),
                hex_head(&result.data, 32)
            );
        }
        Operation::Write {
            interface_number,
            payload,
        } => {
            let result = session.write_report(
                *interface_number,
                config.protocol_timeout_ms(),
                payload.as_slice(),
            )?;
            print_status("WRITE", result.status);
            println!(
                "WRITE os_error={} bytes_written={}",
                result.os_error, result.bytes_written
            );
        }
        Operation::SetFeature {
            interface_number,
            payload,
        } => {
            let result = session.set_feature(
                *interface_number,
                config.protocol_timeout_ms(),
                payload.as_slice(),
            )?;
            print_status("SET_FEATURE", result.status);
            println!(
                "SET_FEATURE os_error={} bytes_accepted={}",
                result.os_error, result.bytes_accepted
            );
        }
        Operation::SetOutput {
            interface_number,
            payload,
        } => {
            let result = session.set_output(
                *interface_number,
                config.protocol_timeout_ms(),
                payload.as_slice(),
            )?;
            print_status("SET_OUTPUT", result.status);
            println!(
                "SET_OUTPUT os_error={} bytes_accepted={}",
                result.os_error, result.bytes_accepted
            );
        }
    }

    Ok(())
}

fn print_identity(session: &GuestSession) {
    let identity = session.identity();
    println!(
        "IDENTITY vid=0x{:04X} pid=0x{:04X} version=0x{:04X} serial={} product={:?}",
        identity.vendor_id,
        identity.product_id,
        identity.version_number,
        identity.serial,
        identity.product
    );
    for collection in &identity.collections {
        println!(
            "COLLECTION role={:?} interface={} usage_page=0x{:04X} usage=0x{:04X} in={} out={} feature={}",
            collection.role,
            collection.interface_number,
            collection.usage_page,
            collection.usage,
            collection.input_report_len,
            collection.output_report_len,
            collection.feature_report_len
        );
    }
}

fn print_status(operation: &str, status: StatusCode) {
    println!("{operation} status={status}");
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, GuestDriverError> {
    let mut config = Config::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--reports" => {
                let count = parse_next::<usize>(&mut args, "--reports")?;
                if count == 0 {
                    return Err(message("--reports must be greater than 0"));
                }
                config.operations.push(Operation::Reports(count));
            }
            "--timeout-ms" => {
                let timeout_ms = parse_next::<u64>(&mut args, "--timeout-ms")?;
                if timeout_ms == 0 || timeout_ms > u16::MAX as u64 {
                    return Err(message("--timeout-ms must be in 1..=65535"));
                }
                config.timeout_ms = timeout_ms;
            }
            "--control-port" => {
                config.control_port = parse_next(&mut args, "--control-port")?;
            }
            "--input-port" => {
                config.input_port = parse_next(&mut args, "--input-port")?;
            }
            "--reconnect" => {
                let count = parse_next::<usize>(&mut args, "--reconnect")?;
                if count == 0 {
                    return Err(message("--reconnect must be greater than 0"));
                }
                config.connect_count = count;
            }
            "--allow-input-timeout" => config.allow_input_timeout = true,
            "--get-feature" => {
                let interface_number = parse_next(&mut args, "--get-feature <interface>")?;
                let report_id = parse_u8(&next_arg(&mut args, "--get-feature <report-id>")?)?;
                let len = parse_next(&mut args, "--get-feature <len>")?;
                config.operations.push(Operation::GetFeature {
                    interface_number,
                    report_id,
                    len,
                });
            }
            "--write-hex" => {
                let interface_number = parse_next(&mut args, "--write-hex <interface>")?;
                let payload = parse_hex(&next_arg(&mut args, "--write-hex <hex>")?)?;
                config.operations.push(Operation::Write {
                    interface_number,
                    payload,
                });
            }
            "--set-feature-hex" => {
                let interface_number = parse_next(&mut args, "--set-feature-hex <interface>")?;
                let payload = parse_hex(&next_arg(&mut args, "--set-feature-hex <hex>")?)?;
                config.operations.push(Operation::SetFeature {
                    interface_number,
                    payload,
                });
            }
            "--set-output-hex" => {
                let interface_number = parse_next(&mut args, "--set-output-hex <interface>")?;
                let payload = parse_hex(&next_arg(&mut args, "--set-output-hex <hex>")?)?;
                config.operations.push(Operation::SetOutput {
                    interface_number,
                    payload,
                });
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            unknown => {
                return Err(message(format!(
                    "unknown guest-driver argument: {unknown}\n\nUse `guest-driver --help`."
                )));
            }
        }
    }

    Ok(config)
}

pub(crate) fn print_help() {
    println!(
        r#"crosspuck-host guest-driver

Runs a local guest-driver smoke client using crosspuck_core::guest, the same
shared transport runtime intended for the real hid.dll proxy.

Usage:
  cargo run -p crosspuck-cli -- guest-driver [options]
  crosspuck-host guest-driver [options]

Options:
  --control-port <port>                 control channel port (default: 28473)
  --input-port <port>                   input channel port (default: 28474)
  --timeout-ms <ms>                     connect/read/write timeout (default: 2000)
  --reconnect <n>                       connect, run operations, disconnect n times
  --allow-input-timeout                 treat input read timeout as success for --reports
  --reports <n>                         read n input reports
  --get-feature <interface> <id> <len>  send GET_FEATURE
  --write-hex <interface> <hex>         send WRITE bytes
  --set-feature-hex <interface> <hex>   send SET_FEATURE bytes
  --set-output-hex <interface> <hex>    send SET_OUTPUT bytes
  -h, --help                            print this help

Operations run in CLI argument order. Hex accepts forms like 82030000,
"82 03 00 00", "82:03:00:00", or "0x82".

Examples:
  cargo run -p crosspuck-cli -- guest-driver
  cargo run -p crosspuck-cli -- guest-driver --get-feature 2 0x02 64
  cargo run -p crosspuck-cli -- guest-driver --reports 1 --allow-input-timeout
  cargo run -p crosspuck-cli -- guest-driver --write-hex 2 82030000
  cargo run -p crosspuck-cli -- guest-driver --reconnect 3
"#
    );
}

fn parse_next<T>(
    args: &mut impl Iterator<Item = String>,
    label: &str,
) -> Result<T, GuestDriverError>
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
) -> Result<String, GuestDriverError> {
    args.next()
        .ok_or_else(|| message(format!("missing value for {label}")))
}

fn parse_u8(value: &str) -> Result<u8, GuestDriverError> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u8::from_str_radix(hex, 16).map_err(|error| message(format!("invalid u8 {value}: {error}")))
    } else {
        value
            .parse::<u8>()
            .map_err(|error| message(format!("invalid u8 {value}: {error}")))
    }
}

fn parse_hex(value: &str) -> Result<Vec<u8>, GuestDriverError> {
    let tokens = value
        .split(|ch: char| ch.is_ascii_whitespace() || matches!(ch, ':' | '-' | ','))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    if tokens.len() > 1 {
        return tokens.into_iter().map(parse_hex_byte).collect();
    }

    let compact = value.trim();
    let compact = compact
        .strip_prefix("0x")
        .or_else(|| compact.strip_prefix("0X"))
        .unwrap_or(compact);

    if compact.is_empty() {
        return Err(message("hex string must not be empty"));
    }
    if !compact.len().is_multiple_of(2) {
        return Err(message("hex string must contain an even number of digits"));
    }

    let mut bytes = Vec::with_capacity(compact.len() / 2);
    for index in (0..compact.len()).step_by(2) {
        let byte = u8::from_str_radix(&compact[index..index + 2], 16)
            .map_err(|error| message(format!("invalid hex byte at offset {index}: {error}")))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn parse_hex_byte(value: &str) -> Result<u8, GuestDriverError> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.is_empty() || value.len() > 2 {
        return Err(message(format!("invalid hex byte {value:?}")));
    }
    u8::from_str_radix(value, 16)
        .map_err(|error| message(format!("invalid hex byte {value}: {error}")))
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

fn message(message: impl Into<String>) -> GuestDriverError {
    GuestDriverError::Message(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parse_preserves_operation_order() {
        let config = parse_args(strings(&[
            "--timeout-ms",
            "1500",
            "--control-port",
            "30001",
            "--input-port",
            "30002",
            "--reconnect",
            "2",
            "--allow-input-timeout",
            "--reports",
            "1",
            "--get-feature",
            "2",
            "0x02",
            "64",
            "--write-hex",
            "2",
            "82030000",
            "--set-feature-hex",
            "2",
            "01:83:01:00",
            "--set-output-hex",
            "2",
            "80 00 00 00",
        ]))
        .unwrap();

        assert_eq!(config.timeout_ms, 1500);
        assert_eq!(config.control_port, 30001);
        assert_eq!(config.input_port, 30002);
        assert_eq!(config.connect_count, 2);
        assert!(config.allow_input_timeout);
        assert_eq!(
            config.operations,
            vec![
                Operation::Reports(1),
                Operation::GetFeature {
                    interface_number: 2,
                    report_id: 0x02,
                    len: 64,
                },
                Operation::Write {
                    interface_number: 2,
                    payload: vec![0x82, 0x03, 0x00, 0x00],
                },
                Operation::SetFeature {
                    interface_number: 2,
                    payload: vec![0x01, 0x83, 0x01, 0x00],
                },
                Operation::SetOutput {
                    interface_number: 2,
                    payload: vec![0x80, 0x00, 0x00, 0x00],
                },
            ]
        );
    }

    #[test]
    fn parse_hex_accepts_common_forms() {
        assert_eq!(parse_hex("82030000").unwrap(), vec![0x82, 0x03, 0x00, 0x00]);
        assert_eq!(
            parse_hex("82:03:00:00").unwrap(),
            vec![0x82, 0x03, 0x00, 0x00]
        );
        assert_eq!(
            parse_hex("0x82 0x03 0 00").unwrap(),
            vec![0x82, 0x03, 0x00, 0x00]
        );
    }

    #[test]
    fn parse_rejects_invalid_values() {
        assert!(parse_args(strings(&["--timeout-ms", "0"])).is_err());
        assert!(parse_args(strings(&["--reconnect", "0"])).is_err());
        assert!(parse_hex("820").is_err());
        assert!(parse_hex("0x100 00").is_err());
    }
}
