mod guest_driver;
mod haptic_loop;

use crosspuck_core::hid::{
    build_puck_snapshot, collect_candidates as collect_core_candidates, HidFilter,
};
use hidapi::{HidApi, HidDevice, HidError};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::CString;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_VALVE_VENDOR_ID: u16 = 0x28DE;
const DEFAULT_READ_SIZE: usize = 64;
const DEFAULT_TIMEOUT_MS: i32 = 1000;

#[derive(Debug)]
struct Config {
    vendor_id: u16,
    product_id: Option<u16>,
    interface_number: Option<i32>,
    usage_page: Option<u16>,
    usage: Option<u16>,
    path: Option<String>,
    serial: Option<String>,
    read_size: usize,
    timeout_ms: i32,
    count: Option<u64>,
    duration_ms: Option<u64>,
    output_path: Option<String>,
    verify_path: Option<String>,
    analyze_hid_probe_path: Option<String>,
    analyze_max_events: usize,
    min_packets: u64,
    quiet: bool,
    probe_all: bool,
    list_only: bool,
    identity_json: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            vendor_id: DEFAULT_VALVE_VENDOR_ID,
            product_id: None,
            interface_number: None,
            usage_page: None,
            usage: None,
            path: None,
            serial: None,
            read_size: DEFAULT_READ_SIZE,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            count: None,
            duration_ms: None,
            output_path: None,
            verify_path: None,
            analyze_hid_probe_path: None,
            analyze_max_events: 40,
            min_packets: 1,
            quiet: false,
            probe_all: false,
            list_only: false,
            identity_json: false,
        }
    }
}

#[derive(Debug)]
enum CliError {
    Message(String),
    GuestDriver(guest_driver::GuestDriverError),
    HapticLoop(haptic_loop::HapticLoopError),
    Hid(HidError),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Message(message) => write!(f, "{message}"),
            CliError::GuestDriver(error) => write!(f, "{error}"),
            CliError::HapticLoop(error) => write!(f, "{error}"),
            CliError::Hid(error) => write!(f, "{error}"),
            CliError::Io(error) => write!(f, "{error}"),
            CliError::Json(error) => write!(f, "{error}"),
        }
    }
}

impl From<guest_driver::GuestDriverError> for CliError {
    fn from(value: guest_driver::GuestDriverError) -> Self {
        CliError::GuestDriver(value)
    }
}

impl From<haptic_loop::HapticLoopError> for CliError {
    fn from(value: haptic_loop::HapticLoopError) -> Self {
        CliError::HapticLoop(value)
    }
}

impl From<HidError> for CliError {
    fn from(value: HidError) -> Self {
        CliError::Hid(value)
    }
}

impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        CliError::Io(value)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(value: serde_json::Error) -> Self {
        CliError::Json(value)
    }
}

type Result<T> = std::result::Result<T, CliError>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("오류: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if matches!(
        args.first().map(String::as_str),
        Some("guest-driver" | "mock-guest")
    ) {
        args.remove(0);
        return guest_driver::run(args).map_err(Into::into);
    }
    if matches!(args.first().map(String::as_str), Some("haptic-loop")) {
        args.remove(0);
        return haptic_loop::run(args).map_err(Into::into);
    }

    let config = parse_args(args.into_iter())?;

    if let Some(path) = &config.verify_path {
        return verify_capture_file(path, &config);
    }

    if let Some(path) = &config.analyze_hid_probe_path {
        return analyze_hid_probe_file(path, &config);
    }

    if config.identity_json {
        return print_identity_json(&config);
    }

    println!("=== [Host PoC] Valve 하드웨어 Raw HID 캡처 시작 ===");
    let api = HidApi::new()?;

    let candidates = collect_candidates(&api, &config);
    print_devices(&candidates);

    if config.list_only {
        return Ok(());
    }

    if config.probe_all {
        return probe_all(&api, &candidates, &config);
    }

    let target = choose_target(&candidates)?;
    println!(
        "대상 장치 선택: VID=0x{:04X}, PID=0x{:04X}, Interface={}, Path={}",
        target.vendor_id, target.product_id, target.interface_number, target.path
    );

    let device = open_target(&api, target)?;
    read_packets(&device, target, &config)
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Config> {
    let mut config = Config::default();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--list" => config.list_only = true,
            "--vendor-id" | "--vid" => {
                config.vendor_id = parse_u16_arg(&arg, args.next())?;
            }
            "--product-id" | "--pid" => {
                config.product_id = Some(parse_u16_arg(&arg, args.next())?);
            }
            "--interface" => {
                config.interface_number = Some(parse_i32_arg(&arg, args.next())?);
            }
            "--usage-page" => {
                config.usage_page = Some(parse_u16_arg(&arg, args.next())?);
            }
            "--usage" => {
                config.usage = Some(parse_u16_arg(&arg, args.next())?);
            }
            "--path" => {
                config.path = Some(require_value(&arg, args.next())?);
            }
            "--serial" => {
                config.serial = Some(require_value(&arg, args.next())?);
            }
            "--identity-json" => config.identity_json = true,
            "--read-size" => {
                let value = parse_usize_arg(&arg, args.next())?;
                if value == 0 {
                    return Err(CliError::Message(
                        "--read-size must be greater than 0".into(),
                    ));
                }
                config.read_size = value;
            }
            "--timeout-ms" => {
                let value = parse_i32_arg(&arg, args.next())?;
                if value < 0 {
                    return Err(CliError::Message(
                        "--timeout-ms must be 0 or greater".into(),
                    ));
                }
                config.timeout_ms = value;
            }
            "--count" => {
                let value = parse_u64_arg(&arg, args.next())?;
                if value == 0 {
                    return Err(CliError::Message("--count must be greater than 0".into()));
                }
                config.count = Some(value);
            }
            "--probe-all" => config.probe_all = true,
            "--quiet" => config.quiet = true,
            "--output" => {
                config.output_path = Some(require_value(&arg, args.next())?);
            }
            "--verify" => {
                config.verify_path = Some(require_value(&arg, args.next())?);
            }
            "--analyze-hid-probe" => {
                config.analyze_hid_probe_path = Some(require_value(&arg, args.next())?);
            }
            "--analyze-max-events" => {
                let value = parse_usize_arg(&arg, args.next())?;
                if value == 0 {
                    return Err(CliError::Message(
                        "--analyze-max-events must be greater than 0".into(),
                    ));
                }
                config.analyze_max_events = value;
            }
            "--min-packets" => {
                let value = parse_u64_arg(&arg, args.next())?;
                if value == 0 {
                    return Err(CliError::Message(
                        "--min-packets must be greater than 0".into(),
                    ));
                }
                config.min_packets = value;
            }
            "--duration-ms" => {
                let value = parse_u64_arg(&arg, args.next())?;
                if value == 0 {
                    return Err(CliError::Message(
                        "--duration-ms must be greater than 0".into(),
                    ));
                }
                config.duration_ms = Some(value);
            }
            unknown => {
                return Err(CliError::Message(format!(
                    "알 수 없는 옵션입니다: {unknown}\n\n도움말은 --help를 사용하십시오."
                )));
            }
        }
    }

    Ok(config)
}

fn print_help() {
    println!(
        r#"crosspuck-host

macOS 호스트에서 Steam Controller 계열 Valve HID 장치를 열고 Raw HID 패킷을 Hex로 출력합니다.
또는 `guest-driver`/`haptic-loop` 서브커맨드로 host app에 연결하는 로컬 guest test client를 실행합니다.

Usage:
  cargo run -- [options]
  cargo run -p crosspuck-cli -- guest-driver [options]
  cargo run -p crosspuck-cli -- haptic-loop [options]

Options:
  --list                 장치 목록만 출력하고 종료
  --vid <hex|dec>        Vendor ID 필터 (기본값: 0x28DE)
  --pid <hex|dec>        Product ID 필터
  --interface <number>   HID interface number 필터
  --usage-page <hex|dec> HID usage page 필터
  --usage <hex|dec>      HID usage 필터
  --path <path>          특정 HID path 직접 지정
  --serial <serial>      특정 Puck serial 필터
  --identity-json        host가 관측한 Puck identity/collection snapshot을 JSON으로 출력
  --read-size <bytes>    Read buffer 크기 (기본값: 64)
  --timeout-ms <ms>      read_timeout 대기 시간 (기본값: 1000)
  --count <n>            n개 패킷 캡처 후 종료
  --probe-all            필터에 매칭되는 모든 unique HID path를 열고 어느 path가 움직이는지 탐색
  --duration-ms <ms>     지정한 시간 동안만 실행 후 종료
  --output <file>        캡처 결과를 JSONL 파일로 저장
  --verify <file>        JSONL 캡처 파일을 검증하고 종료
  --analyze-hid-probe <file>
                         macOS native Steam HID probe JSONL을 분석하고 종료
  --analyze-max-events <n>
                         scenario별 출력할 host->device event 최대 개수 (기본값: 40)
  --min-packets <n>      verify 시 필요한 최소 packet 수 (기본값: 1)
  --quiet                캡처 중 packet별 콘솔 출력을 생략
  guest-driver           host app transport에 연결하는 local mock guest 실행
  haptic-loop            버튼 입력 시 짧은 Triton rumble feedback을 보내는 local guest test 실행
  -h, --help             도움말 출력

Examples:
  cargo run -- --list
  cargo run -- --pid 0x1142 --count 20
  cargo run -- --pid 0x1304 --interface 6 --duration-ms 3000
  cargo run -- --pid 0x1304 --usage-page 0xFF00 --duration-ms 3000
  cargo run -- --pid 0x1304 --probe-all --duration-ms 10000
  cargo run -- --pid 0x1304 --identity-json
  cargo run -- --pid 0x1304 --duration-ms 5000 --output captures/manual.jsonl --quiet
  cargo run -- --verify captures/manual.jsonl --min-packets 20
  cargo run -- --analyze-hid-probe captures/native_feedback_20260523-120000.jsonl
  cargo run -- --interface 1 --timeout-ms 250
  cargo run -p crosspuck-cli -- guest-driver --get-feature 2 0x02 64
  cargo run -p crosspuck-cli -- guest-driver --reports 1 --allow-input-timeout
  cargo run -p crosspuck-cli -- haptic-loop --duration-ms 30000
"#
    );
}

fn print_identity_json(config: &Config) -> Result<()> {
    let api = HidApi::new()?;
    let filter = HidFilter {
        vendor_id: Some(config.vendor_id),
        product_id: config.product_id,
        serial: config.serial.clone(),
    };
    let candidates = collect_core_candidates(&api, &filter);
    let snapshot = build_puck_snapshot(&candidates)
        .map_err(|error| CliError::Message(format!("identity snapshot 실패: {error}")))?;

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &snapshot)?;
    handle.write_all(b"\n")?;
    Ok(())
}

#[derive(Debug, Clone)]
struct DeviceCandidate {
    path: String,
    vendor_id: u16,
    product_id: u16,
    interface_number: i32,
    usage_page: u16,
    usage: u16,
    manufacturer: Option<String>,
    product: Option<String>,
    serial: Option<String>,
}

fn collect_candidates(api: &HidApi, config: &Config) -> Vec<DeviceCandidate> {
    api.device_list()
        .filter(|device| device.vendor_id() == config.vendor_id)
        .filter(|device| {
            config
                .product_id
                .is_none_or(|product_id| device.product_id() == product_id)
        })
        .filter(|device| {
            config
                .interface_number
                .is_none_or(|interface_number| device.interface_number() == interface_number)
        })
        .filter(|device| {
            config
                .usage_page
                .is_none_or(|usage_page| device.usage_page() == usage_page)
        })
        .filter(|device| config.usage.is_none_or(|usage| device.usage() == usage))
        .filter(|device| {
            config
                .serial
                .as_ref()
                .is_none_or(|serial| device.serial_number().is_some_and(|value| value == serial))
        })
        .filter(|device| {
            config
                .path
                .as_ref()
                .is_none_or(|path| device.path().to_string_lossy().as_ref() == path.as_str())
        })
        .map(|device| DeviceCandidate {
            path: device.path().to_string_lossy().into_owned(),
            vendor_id: device.vendor_id(),
            product_id: device.product_id(),
            interface_number: device.interface_number(),
            usage_page: device.usage_page(),
            usage: device.usage(),
            manufacturer: device.manufacturer_string().map(ToOwned::to_owned),
            product: device.product_string().map(ToOwned::to_owned),
            serial: device.serial_number().map(ToOwned::to_owned),
        })
        .collect()
}

fn print_devices(candidates: &[DeviceCandidate]) {
    if candidates.is_empty() {
        println!("검출된 Valve HID 장치가 없습니다.");
        return;
    }

    println!("검출된 Valve HID 장치: {}개", candidates.len());
    for (index, device) in candidates.iter().enumerate() {
        println!(
            "[{index}] VID=0x{:04X} PID=0x{:04X} Interface={} UsagePage=0x{:04X} Usage=0x{:04X}",
            device.vendor_id,
            device.product_id,
            device.interface_number,
            device.usage_page,
            device.usage
        );
        println!("    Path: {}", device.path);
        println!(
            "    Manufacturer: {} | Product: {} | Serial: {}",
            device.manufacturer.as_deref().unwrap_or("-"),
            device.product.as_deref().unwrap_or("-"),
            device.serial.as_deref().unwrap_or("-")
        );
    }
}

fn choose_target(candidates: &[DeviceCandidate]) -> Result<&DeviceCandidate> {
    candidates
        .iter()
        .max_by_key(|device| target_score(device))
        .or_else(|| candidates.first())
        .ok_or_else(|| {
            CliError::Message(
                "Steam Controller 2 후보 장치를 찾을 수 없습니다. --list로 장치 정보를 확인하십시오."
                .into(),
        )
        })
}

fn target_score(device: &DeviceCandidate) -> u16 {
    let observed_puck_report_stream =
        u16::from(device.interface_number == 2 && device.usage_page == 0x0001) * 200;
    let generic_desktop_mouse =
        u16::from(device.usage_page == 0x0001 && device.usage == 0x0002) * 75;
    let vendor_specific = u16::from(device.usage_page == 0xFF00) * 50;
    let raw_channel = u16::from(device.usage_page == 0xFF00 && device.usage == 0x0002) * 25;

    observed_puck_report_stream + generic_desktop_mouse + vendor_specific + raw_channel
}

fn open_target(api: &HidApi, target: &DeviceCandidate) -> Result<HidDevice> {
    let path = CString::new(target.path.as_bytes()).map_err(|_| {
        CliError::Message("HID path에 NUL 바이트가 포함되어 열 수 없습니다.".into())
    })?;

    api.open_path(path.as_c_str()).map_err(|error| {
        CliError::Message(format!(
            "장치 개방 실패: {error}\nmacOS 입력 모니터링/블루투스 권한 또는 다른 프로세스의 독점 점유 상태를 확인하십시오."
        ))
    })
}

struct ProbeState {
    label: String,
    device: HidDevice,
    packets_seen: u64,
    read_error_seen: bool,
}

fn probe_all(api: &HidApi, candidates: &[DeviceCandidate], config: &Config) -> Result<()> {
    let unique_candidates = unique_by_path(candidates);
    if unique_candidates.is_empty() {
        return Err(CliError::Message(
            "probe할 Valve HID 장치가 없습니다. --list로 장치 정보를 확인하십시오.".into(),
        ));
    }

    println!(
        "Probe 시작: unique path={} duration={}ms immediate polling",
        unique_candidates.len(),
        probe_duration(config).as_millis()
    );

    let mut states = Vec::new();
    for (index, candidate) in unique_candidates.into_iter().enumerate() {
        let label = format!(
            "[probe {index}] PID=0x{:04X} Interface={} UsagePage=0x{:04X} Usage=0x{:04X} Path={}",
            candidate.product_id,
            candidate.interface_number,
            candidate.usage_page,
            candidate.usage,
            candidate.path
        );

        match open_target(api, candidate) {
            Ok(device) => {
                println!("OPEN  {label}");
                states.push(ProbeState {
                    label,
                    device,
                    packets_seen: 0,
                    read_error_seen: false,
                });
            }
            Err(error) => {
                println!("SKIP  {label}");
                println!("      {error}");
            }
        }
    }

    if states.is_empty() {
        return Err(CliError::Message(
            "열 수 있는 HID path가 없습니다. macOS 권한 또는 독점 점유 상태를 확인하십시오.".into(),
        ));
    }

    println!(
        "열린 path={}개. 컨트롤러를 조작하십시오. 입력이 발생하는 path가 있으면 hex report를 출력합니다.",
        states.len()
    );

    let started_at = Instant::now();
    let stop_after = probe_duration(config);
    let mut total_packets = 0_u64;
    let mut buffer = vec![0_u8; config.read_size];

    while started_at.elapsed() < stop_after {
        for state in &mut states {
            match state.device.read_timeout(&mut buffer, 0) {
                Ok(bytes_read) if bytes_read > 0 => {
                    total_packets += 1;
                    state.packets_seen += 1;
                    println!(
                        "[{:>8.3}s #{:<6}] {}  {:>3} bytes  {}",
                        elapsed_seconds(started_at.elapsed()),
                        state.packets_seen,
                        state.label,
                        bytes_read,
                        format_hex(&buffer[..bytes_read])
                    );

                    if config.count.is_some_and(|count| total_packets >= count) {
                        print_probe_summary(&states, total_packets);
                        return Ok(());
                    }
                }
                Ok(_) => {}
                Err(error) if !state.read_error_seen => {
                    state.read_error_seen = true;
                    println!("READ-ERR {}  {}", state.label, error);
                }
                Err(_) => {}
            }
        }

        sleep(Duration::from_millis(5));
    }

    print_probe_summary(&states, total_packets);
    Ok(())
}

fn unique_by_path(candidates: &[DeviceCandidate]) -> Vec<&DeviceCandidate> {
    let mut unique = Vec::new();
    for candidate in candidates {
        if unique
            .iter()
            .any(|existing: &&DeviceCandidate| existing.path == candidate.path)
        {
            continue;
        }
        unique.push(candidate);
    }
    unique.sort_by_key(|candidate| std::cmp::Reverse(target_score(candidate)));
    unique
}

fn probe_duration(config: &Config) -> Duration {
    Duration::from_millis(config.duration_ms.unwrap_or(10_000))
}

fn print_probe_summary(states: &[ProbeState], total_packets: u64) {
    println!("Probe 종료. 총 캡처 패킷: {total_packets}개");
    for state in states {
        println!("  {:>4} packets  {}", state.packets_seen, state.label);
    }
}

struct CaptureWriter {
    writer: BufWriter<File>,
}

impl CaptureWriter {
    fn new(path: &str, target: &DeviceCandidate, config: &Config) -> Result<Self> {
        create_parent_dir(path)?;

        let file = File::create(path)?;
        let mut capture = Self {
            writer: BufWriter::new(file),
        };

        capture.write_value(json!({
            "type": "metadata",
            "schema": "crosspuck.host.capture.v1",
            "started_unix_ms": unix_time_ms(),
            "device": {
                "path": target.path.as_str(),
                "vendor_id": format!("0x{:04X}", target.vendor_id),
                "product_id": format!("0x{:04X}", target.product_id),
                "interface_number": target.interface_number,
                "usage_page": format!("0x{:04X}", target.usage_page),
                "usage": format!("0x{:04X}", target.usage),
                "manufacturer": target.manufacturer.as_deref(),
                "product": target.product.as_deref(),
                "serial": target.serial.as_deref(),
            },
            "config": {
                "read_size": config.read_size,
                "timeout_ms": config.timeout_ms,
                "count": config.count,
                "duration_ms": config.duration_ms,
            }
        }))?;

        Ok(capture)
    }

    fn write_packet(&mut self, seq: u64, elapsed: Duration, bytes: &[u8]) -> Result<()> {
        self.write_value(json!({
            "type": "packet",
            "seq": seq,
            "elapsed_us": elapsed.as_micros(),
            "bytes_read": bytes.len(),
            "hex": format_hex(bytes),
        }))
    }

    fn write_summary(
        &mut self,
        elapsed: Duration,
        packets_seen: u64,
        total_timeouts: u64,
    ) -> Result<()> {
        self.write_value(json!({
            "type": "summary",
            "elapsed_ms": elapsed.as_millis(),
            "packets": packets_seen,
            "timeouts": total_timeouts,
            "completed": true,
        }))?;
        self.writer.flush()?;
        Ok(())
    }

    fn write_value(&mut self, value: Value) -> Result<()> {
        serde_json::to_writer(&mut self.writer, &value)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }
}

fn read_packets(device: &HidDevice, target: &DeviceCandidate, config: &Config) -> Result<()> {
    println!(
        "성공: 하드웨어 스트림에 바인딩되었습니다. read_size={} timeout={}ms count={} duration={}",
        config.read_size,
        config.timeout_ms,
        config
            .count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "무제한".to_string()),
        config
            .duration_ms
            .map(|duration_ms| format!("{duration_ms}ms"))
            .unwrap_or_else(|| "무제한".to_string())
    );
    println!("컨트롤러 버튼, 트랙패드, 자이로 등을 조작하십시오. 종료: Ctrl+C");

    let mut capture = match config.output_path.as_deref() {
        Some(path) => {
            println!("캡처 파일 저장: {path}");
            Some(CaptureWriter::new(path, target, config)?)
        }
        None => None,
    };

    let mut buffer = vec![0_u8; config.read_size];
    let started_at = Instant::now();
    let mut packets_seen = 0_u64;
    let mut timeout_ticks = 0_u64;
    let mut total_timeouts = 0_u64;

    loop {
        match device.read_timeout(&mut buffer, config.timeout_ms)? {
            bytes_read if bytes_read > 0 => {
                packets_seen += 1;
                timeout_ticks = 0;
                if let Some(capture) = capture.as_mut() {
                    capture.write_packet(
                        packets_seen,
                        started_at.elapsed(),
                        &buffer[..bytes_read],
                    )?;
                }
                if !config.quiet {
                    println!(
                        "[{:>8.3}s #{:<6}] {:>3} bytes  {}",
                        elapsed_seconds(started_at.elapsed()),
                        packets_seen,
                        bytes_read,
                        format_hex(&buffer[..bytes_read])
                    );
                }

                if config.count.is_some_and(|count| packets_seen >= count) {
                    if let Some(capture) = capture.as_mut() {
                        capture.write_summary(
                            started_at.elapsed(),
                            packets_seen,
                            total_timeouts,
                        )?;
                    }
                    println!("요청한 {packets_seen}개 패킷을 캡처하고 종료합니다.");
                    return Ok(());
                }
            }
            _ => {
                timeout_ticks += 1;
                total_timeouts += 1;
                if !config.quiet && (timeout_ticks == 1 || timeout_ticks.is_multiple_of(10)) {
                    println!(
                        "[{:>8.3}s] 대기 중... 패킷 없음 ({}ms timeout)",
                        elapsed_seconds(started_at.elapsed()),
                        config.timeout_ms
                    );
                }
            }
        }

        if config
            .duration_ms
            .is_some_and(|duration_ms| started_at.elapsed() >= Duration::from_millis(duration_ms))
        {
            if let Some(capture) = capture.as_mut() {
                capture.write_summary(started_at.elapsed(), packets_seen, total_timeouts)?;
            }
            println!(
                "지정한 실행 시간이 지나 종료합니다. 캡처된 패킷: {}개",
                packets_seen
            );
            return Ok(());
        }
    }
}

fn verify_capture_file(path: &str, config: &Config) -> Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut metadata_seen = false;
    let mut summary_packets = None;
    let mut summary_elapsed_ms = None;
    let mut packet_count = 0_u64;
    let mut expected_seq = 1_u64;
    let mut last_elapsed_us = None;
    let mut size_counts = BTreeMap::<u64, u64>::new();
    let mut unique_payloads = BTreeSet::<String>::new();
    let mut errors = Vec::new();

    for (line_index, line) in reader.lines().enumerate() {
        let line_number = line_index + 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                errors.push(format!("line {line_number}: JSON 파싱 실패: {error}"));
                continue;
            }
        };

        match value.get("type").and_then(Value::as_str) {
            Some("metadata") => {
                metadata_seen = true;
                if value.get("schema").and_then(Value::as_str) != Some("crosspuck.host.capture.v1")
                {
                    errors.push(format!("line {line_number}: 알 수 없는 schema"));
                }
            }
            Some("packet") => {
                let seq = value.get("seq").and_then(Value::as_u64);
                let elapsed_us = value.get("elapsed_us").and_then(Value::as_u64);
                let bytes_read = value.get("bytes_read").and_then(Value::as_u64);
                let hex = value.get("hex").and_then(Value::as_str);

                let Some(seq) = seq else {
                    errors.push(format!("line {line_number}: packet.seq 누락"));
                    continue;
                };
                let Some(elapsed_us) = elapsed_us else {
                    errors.push(format!("line {line_number}: packet.elapsed_us 누락"));
                    continue;
                };
                let Some(bytes_read) = bytes_read else {
                    errors.push(format!("line {line_number}: packet.bytes_read 누락"));
                    continue;
                };
                let Some(hex) = hex else {
                    errors.push(format!("line {line_number}: packet.hex 누락"));
                    continue;
                };

                if seq != expected_seq {
                    errors.push(format!(
                        "line {line_number}: seq 불연속, expected={expected_seq} actual={seq}"
                    ));
                    expected_seq = seq;
                }
                expected_seq += 1;

                if last_elapsed_us.is_some_and(|last| elapsed_us < last) {
                    errors.push(format!("line {line_number}: elapsed_us가 감소했습니다"));
                }
                last_elapsed_us = Some(elapsed_us);

                match parse_hex_bytes(hex) {
                    Ok(bytes) => {
                        if bytes.len() as u64 != bytes_read {
                            errors.push(format!(
                                "line {line_number}: bytes_read={bytes_read}, hex byte count={}",
                                bytes.len()
                            ));
                        }
                    }
                    Err(error) => errors.push(format!("line {line_number}: {error}")),
                }

                if bytes_read == 0 {
                    errors.push(format!("line {line_number}: 빈 packet"));
                }

                *size_counts.entry(bytes_read).or_default() += 1;
                unique_payloads.insert(hex.to_string());
                packet_count += 1;
            }
            Some("summary") => {
                summary_packets = value.get("packets").and_then(Value::as_u64);
                summary_elapsed_ms = value.get("elapsed_ms").and_then(Value::as_u64);
            }
            Some(other) => errors.push(format!("line {line_number}: 알 수 없는 type={other}")),
            None => errors.push(format!("line {line_number}: type 누락")),
        }
    }

    if !metadata_seen {
        errors.push("metadata record가 없습니다".to_string());
    }

    if packet_count < config.min_packets {
        errors.push(format!(
            "packet 수가 부족합니다: min={} actual={}",
            config.min_packets, packet_count
        ));
    }

    match summary_packets {
        Some(summary_packets) if summary_packets != packet_count => {
            errors.push(format!(
                "summary.packets와 실제 packet 수가 다릅니다: summary={summary_packets} actual={packet_count}"
            ));
        }
        Some(_) => {}
        None => errors
            .push("summary record가 없습니다. 정상 종료된 캡처 파일인지 확인하십시오".to_string()),
    }

    let dominant_size = size_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(size, count)| (*size, *count));

    if errors.is_empty() {
        println!("검증 성공: {path}");
        println!("  packets: {packet_count}");
        println!(
            "  elapsed: {}ms",
            summary_elapsed_ms
                .map(|elapsed| elapsed.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        if let Some((size, count)) = dominant_size {
            println!("  dominant report size: {size} bytes ({count} packets)");
        }
        println!("  unique payloads: {}", unique_payloads.len());
        return Ok(());
    }

    println!("검증 실패: {path}");
    for error in &errors {
        println!("  - {error}");
    }
    Err(CliError::Message(format!(
        "캡처 파일 검증 실패: {}개 문제",
        errors.len()
    )))
}

#[derive(Clone, Debug)]
struct HidProbeMarker {
    unix_ms: u64,
    scenario: String,
    phase: String,
    description: String,
}

#[derive(Clone, Debug)]
struct HidProbeFeedbackEvent {
    unix_ms: u64,
    event: String,
    report_type: String,
    report_id: Option<u64>,
    len: Option<u64>,
    hex: Option<String>,
    result: Option<String>,
    device: String,
}

#[derive(Clone, Debug)]
struct ScenarioWindow {
    scenario: String,
    description: String,
    start_ms: u64,
    end_ms: u64,
}

fn analyze_hid_probe_file(path: &str, config: &Config) -> Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut markers = Vec::new();
    let mut feedback_events = Vec::new();
    let mut hid_probe_records = 0_u64;
    let mut skipped_non_json = 0_u64;
    let mut parse_errors = Vec::new();

    for (line_index, line) in reader.lines().enumerate() {
        let line_number = line_index + 1;
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => {
                skipped_non_json += 1;
                continue;
            }
        };

        match value.get("type").and_then(Value::as_str) {
            Some("marker") => match parse_hid_probe_marker(&value) {
                Some(marker) => markers.push(marker),
                None => parse_errors.push(format!("line {line_number}: invalid marker record")),
            },
            Some("hid_probe") => {
                hid_probe_records += 1;
                if let Some(event) = parse_hid_probe_feedback_event(&value) {
                    feedback_events.push(event);
                }
            }
            Some(_) | None => {}
        }
    }

    markers.sort_by_key(|marker| marker.unix_ms);
    feedback_events.sort_by_key(|event| event.unix_ms);

    let windows = scenario_windows(&markers, &feedback_events);

    println!("HID probe 분석: {path}");
    println!("  hid_probe records: {hid_probe_records}");
    println!(
        "  host->device feedback requests: {}",
        feedback_events.len()
    );
    println!("  markers: {}", markers.len());
    if skipped_non_json > 0 {
        println!("  skipped non-JSON lines: {skipped_non_json}");
    }
    if !parse_errors.is_empty() {
        println!("  parse warnings: {}", parse_errors.len());
        for warning in parse_errors.iter().take(10) {
            println!("    - {warning}");
        }
    }

    if feedback_events.is_empty() {
        println!("  host->device feedback request가 없습니다.");
        println!(
            "  DYLD interpose 적용 여부, Steam 재시작 여부, LOG_INPUT/LOG_GET 설정을 확인하십시오."
        );
        return Ok(());
    }

    for window in windows {
        print_scenario_feedback_summary(&window, &feedback_events, config.analyze_max_events);
    }

    Ok(())
}

fn parse_hid_probe_marker(value: &Value) -> Option<HidProbeMarker> {
    Some(HidProbeMarker {
        unix_ms: value.get("unix_ms")?.as_u64()?,
        scenario: value.get("scenario")?.as_str()?.to_string(),
        phase: value.get("phase")?.as_str()?.to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn parse_hid_probe_feedback_event(value: &Value) -> Option<HidProbeFeedbackEvent> {
    if value.get("direction").and_then(Value::as_str) != Some("host_to_device") {
        return None;
    }
    if value.get("phase").and_then(Value::as_str) != Some("request") {
        return None;
    }

    let event = value.get("event")?.as_str()?;
    if !event.starts_with("set_report") && !event.starts_with("set_value") {
        return None;
    }

    let report_id = value.get("report_id").and_then(Value::as_u64).or_else(|| {
        value
            .get("element")
            .and_then(|element| element.get("report_id"))
            .and_then(Value::as_u64)
    });
    let len = value
        .get("len")
        .and_then(Value::as_u64)
        .or_else(|| value.get("value_len").and_then(Value::as_u64));
    let report_type = value
        .get("report_type")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "value".to_string());

    Some(HidProbeFeedbackEvent {
        unix_ms: value.get("unix_ms")?.as_u64()?,
        event: event.to_string(),
        report_type,
        report_id,
        len,
        hex: value
            .get("hex")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        result: value
            .get("result")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        device: device_summary(value.get("device")),
    })
}

fn device_summary(device: Option<&Value>) -> String {
    let Some(device) = device else {
        return "-".to_string();
    };

    let vid = device
        .get("vid_hex")
        .and_then(Value::as_str)
        .unwrap_or("VID?");
    let pid = device
        .get("pid_hex")
        .and_then(Value::as_str)
        .unwrap_or("PID?");
    let usage_page = device
        .get("usage_page_hex")
        .and_then(Value::as_str)
        .unwrap_or("UP?");
    let usage = device
        .get("usage_hex")
        .and_then(Value::as_str)
        .unwrap_or("U?");
    let serial = device
        .get("serial")
        .and_then(Value::as_str)
        .filter(|serial| !serial.is_empty())
        .unwrap_or("-");

    format!("{vid}/{pid} usage={usage_page}:{usage} serial={serial}")
}

fn scenario_windows(
    markers: &[HidProbeMarker],
    feedback_events: &[HidProbeFeedbackEvent],
) -> Vec<ScenarioWindow> {
    let mut windows = Vec::new();
    let mut open = BTreeMap::<String, HidProbeMarker>::new();

    for marker in markers {
        match marker.phase.as_str() {
            "start" if marker.scenario != "capture" => {
                open.insert(marker.scenario.clone(), marker.clone());
            }
            "end" if marker.scenario != "capture" => {
                if let Some(start) = open.remove(&marker.scenario) {
                    windows.push(ScenarioWindow {
                        scenario: marker.scenario.clone(),
                        description: start.description,
                        start_ms: start.unix_ms,
                        end_ms: marker.unix_ms.max(start.unix_ms),
                    });
                }
            }
            _ => {}
        }
    }

    if windows.is_empty() {
        let start_ms = feedback_events
            .first()
            .map(|event| event.unix_ms)
            .unwrap_or(0);
        let end_ms = feedback_events
            .last()
            .map(|event| event.unix_ms)
            .unwrap_or(start_ms);
        windows.push(ScenarioWindow {
            scenario: "all".to_string(),
            description: "no scenario markers".to_string(),
            start_ms,
            end_ms,
        });
    }

    windows.sort_by_key(|window| window.start_ms);
    windows
}

fn print_scenario_feedback_summary(
    window: &ScenarioWindow,
    events: &[HidProbeFeedbackEvent],
    max_events: usize,
) {
    let scoped = events
        .iter()
        .filter(|event| event.unix_ms >= window.start_ms && event.unix_ms <= window.end_ms)
        .collect::<Vec<_>>();
    let duration_ms = window.end_ms.saturating_sub(window.start_ms);

    println!();
    println!(
        "Scenario: {} ({}ms) - {}",
        window.scenario, duration_ms, window.description
    );
    println!("  host->device requests: {}", scoped.len());

    if scoped.is_empty() {
        println!("  이 window 안에는 feedback request가 없습니다.");
        return;
    }

    let mut by_signature = BTreeMap::<String, u64>::new();
    let mut unique_payloads = BTreeSet::<String>::new();
    for event in &scoped {
        *by_signature.entry(event_signature(event)).or_default() += 1;
        if let Some(hex) = &event.hex {
            unique_payloads.insert(hex.clone());
        }
    }

    println!("  unique payloads: {}", unique_payloads.len());
    println!("  request groups:");
    for (signature, count) in by_signature {
        println!("    {:>4}  {signature}", count);
    }

    println!("  ordered events:");
    for event in scoped.iter().take(max_events) {
        let rel_ms = event.unix_ms.saturating_sub(window.start_ms);
        println!(
            "    +{:>6}ms {:<32} type={:<7} id={} len={} result={} {} hex={}",
            rel_ms,
            event.event,
            event.report_type,
            event
                .report_id
                .map(|id| format!("0x{id:02X}"))
                .unwrap_or_else(|| "-".to_string()),
            event
                .len
                .map(|len| len.to_string())
                .unwrap_or_else(|| "-".to_string()),
            event.result.as_deref().unwrap_or("-"),
            event.device,
            event
                .hex
                .as_deref()
                .map(|hex| shorten(hex, 160))
                .unwrap_or_else(|| "-".to_string())
        );
    }
    if scoped.len() > max_events {
        println!("    ... {} more events", scoped.len() - max_events);
    }
}

fn event_signature(event: &HidProbeFeedbackEvent) -> String {
    format!(
        "{} type={} id={} len={}",
        event.event,
        event.report_type,
        event
            .report_id
            .map(|id| format!("0x{id:02X}"))
            .unwrap_or_else(|| "-".to_string()),
        event
            .len
            .map(|len| len.to_string())
            .unwrap_or_else(|| "-".to_string())
    )
}

fn shorten(value: &str, max_chars: usize) -> String {
    if value.len() <= max_chars {
        return value.to_string();
    }
    format!("{} ...", &value[..max_chars])
}

fn create_parent_dir(path: &str) -> Result<()> {
    let Some(parent) = Path::new(path).parent() else {
        return Ok(());
    };

    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }

    Ok(())
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn elapsed_seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn format_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_hex_bytes(hex: &str) -> std::result::Result<Vec<u8>, String> {
    hex.split_whitespace()
        .map(|word| {
            u8::from_str_radix(word, 16)
                .map_err(|_| format!("hex byte를 파싱할 수 없습니다: {word}"))
        })
        .collect()
}

fn require_value(flag: &str, value: Option<String>) -> Result<String> {
    value.ok_or_else(|| CliError::Message(format!("{flag} 옵션에는 값이 필요합니다.")))
}

fn parse_u16_arg(flag: &str, value: Option<String>) -> Result<u16> {
    let raw = require_value(flag, value)?;
    parse_prefixed_u64(&raw)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| CliError::Message(format!("{flag} 값이 u16 범위를 벗어났습니다: {raw}")))
}

fn parse_u64_arg(flag: &str, value: Option<String>) -> Result<u64> {
    let raw = require_value(flag, value)?;
    parse_prefixed_u64(&raw)
        .ok_or_else(|| CliError::Message(format!("{flag} 값을 숫자로 해석할 수 없습니다: {raw}")))
}

fn parse_i32_arg(flag: &str, value: Option<String>) -> Result<i32> {
    let raw = require_value(flag, value)?;
    raw.parse::<i32>()
        .map_err(|_| CliError::Message(format!("{flag} 값을 i32 숫자로 해석할 수 없습니다: {raw}")))
}

fn parse_usize_arg(flag: &str, value: Option<String>) -> Result<usize> {
    let raw = require_value(flag, value)?;
    parse_prefixed_u64(&raw)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            CliError::Message(format!(
                "{flag} 값을 usize 숫자로 해석할 수 없습니다: {raw}"
            ))
        })
}

fn parse_prefixed_u64(raw: &str) -> Option<u64> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse::<u64>().ok()
    }
}
