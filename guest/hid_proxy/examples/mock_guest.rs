use crosspuck_core::guest::{GuestError, GuestSession, GuestTransportClient, GuestTransportConfig};
use crosspuck_core::protocol::StatusCode;
use std::env;
use std::time::Duration;

#[derive(Debug)]
struct Config {
    reports: usize,
    timeout_ms: u64,
    get_feature: Option<(u8, u8, u16)>,
    write: Option<(u8, Vec<u8>)>,
    set_feature: Option<(u8, Vec<u8>)>,
    set_output: Option<(u8, Vec<u8>)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            reports: 0,
            timeout_ms: 2_000,
            get_feature: None,
            write: None,
            set_feature: None,
            set_output: None,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let mut session = GuestTransportClient::connect(GuestTransportConfig {
        io_timeout: Duration::from_millis(config.timeout_ms),
        connect_timeout: Duration::from_millis(config.timeout_ms),
        guest_label: "crosspuck-mock-guest".to_string(),
        ..GuestTransportConfig::default()
    })?;

    print_identity(&session);

    for _ in 0..config.reports {
        let report = session.read_input_report()?;
        println!(
            "INPUT seq={} interface={} role={:?} len={} head={}",
            report.sequence,
            report.interface_number,
            report.role,
            report.data.len(),
            hex_head(&report.data, 16)
        );
    }

    if let Some((interface_number, report_id, len)) = config.get_feature {
        let result =
            session.get_feature(interface_number, report_id, len, config.timeout_ms as u16)?;
        print_status("GET_FEATURE", result.status);
        println!(
            "FEATURE len={} head={}",
            result.data.len(),
            hex_head(&result.data, 32)
        );
    }

    if let Some((interface_number, payload)) = config.write {
        let result = session.write_report(interface_number, config.timeout_ms as u16, &payload)?;
        print_status("WRITE", result.status);
        println!("WRITE bytes_written={}", result.bytes_written);
    }

    if let Some((interface_number, payload)) = config.set_feature {
        let result = session.set_feature(interface_number, config.timeout_ms as u16, &payload)?;
        print_status("SET_FEATURE", result.status);
        println!("SET_FEATURE bytes_accepted={}", result.bytes_accepted);
    }

    if let Some((interface_number, payload)) = config.set_output {
        let result = session.set_output(interface_number, config.timeout_ms as u16, &payload)?;
        print_status("SET_OUTPUT", result.status);
        println!("SET_OUTPUT bytes_accepted={}", result.bytes_accepted);
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

fn parse_args() -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--reports" => {
                config.reports = parse_next(&mut args, "--reports")?;
            }
            "--timeout-ms" => {
                config.timeout_ms = parse_next(&mut args, "--timeout-ms")?;
            }
            "--get-feature" => {
                let interface_number = parse_next(&mut args, "--get-feature <interface>")?;
                let report_id = parse_u8(&next_arg(&mut args, "--get-feature <report-id>")?)?;
                let len = parse_next(&mut args, "--get-feature <len>")?;
                config.get_feature = Some((interface_number, report_id, len));
            }
            "--write-hex" => {
                let interface_number = parse_next(&mut args, "--write-hex <interface>")?;
                let payload = parse_hex(&next_arg(&mut args, "--write-hex <hex>")?)?;
                config.write = Some((interface_number, payload));
            }
            "--set-feature-hex" => {
                let interface_number = parse_next(&mut args, "--set-feature-hex <interface>")?;
                let payload = parse_hex(&next_arg(&mut args, "--set-feature-hex <hex>")?)?;
                config.set_feature = Some((interface_number, payload));
            }
            "--set-output-hex" => {
                let interface_number = parse_next(&mut args, "--set-output-hex <interface>")?;
                let payload = parse_hex(&next_arg(&mut args, "--set-output-hex <hex>")?)?;
                config.set_output = Some((interface_number, payload));
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(config)
}

fn print_help() {
    println!(
        r#"mock_guest

Uses crosspuck_core::guest shared runtime, the same transport client intended for hid.dll.

Usage:
  cargo run --manifest-path guest/hid_proxy/Cargo.toml --example mock_guest -- [options]

Options:
  --reports <n>                         read n input reports
  --timeout-ms <ms>                     connect/read/write timeout
  --get-feature <interface> <id> <len>  send GET_FEATURE
  --write-hex <interface> <hex>         send WRITE bytes
  --set-feature-hex <interface> <hex>   send SET_FEATURE bytes
  --set-output-hex <interface> <hex>    send SET_OUTPUT bytes
"#
    );
}

fn parse_next<T>(args: &mut impl Iterator<Item = String>, label: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = next_arg(args, label)?;
    value
        .parse::<T>()
        .map_err(|error| format!("invalid {label}: {error}"))
}

fn next_arg(args: &mut impl Iterator<Item = String>, label: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {label}"))
}

fn parse_u8(value: &str) -> Result<u8, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u8::from_str_radix(hex, 16).map_err(|error| format!("invalid u8 hex {value}: {error}"))
    } else {
        value
            .parse::<u8>()
            .map_err(|error| format!("invalid u8 {value}: {error}"))
    }
}

fn parse_hex(value: &str) -> Result<Vec<u8>, String> {
    let compact = value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != ':' && *ch != '-')
        .collect::<String>();
    if compact.len() % 2 != 0 {
        return Err("hex string must contain an even number of digits".to_string());
    }

    let mut bytes = Vec::with_capacity(compact.len() / 2);
    for index in (0..compact.len()).step_by(2) {
        let byte = u8::from_str_radix(&compact[index..index + 2], 16)
            .map_err(|error| format!("invalid hex byte at offset {index}: {error}"))?;
        bytes.push(byte);
    }
    Ok(bytes)
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

#[allow(dead_code)]
fn _assert_error_type_is_public(error: GuestError) -> GuestError {
    error
}
