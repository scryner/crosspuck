use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::env;
use std::ffi::CString;
use std::thread::sleep;
use std::time::{Duration, Instant};

const VALVE_VENDOR_ID: u16 = 0x28DE;
const DEFAULT_PID: u16 = 0x1304;
const DEFAULT_INTERFACE: i32 = 2;

const DEFAULT_RUMBLE_DURATION_MS: u64 = 300;
const DEFAULT_RUMBLE_INTERVAL_MS: u64 = 40;
const DEFAULT_EFFECT_MS: u16 = 120;
const DEFAULT_PAUSE_MS: u64 = 180;

const ID_OUT_REPORT_HAPTIC_RUMBLE: u8 = 0x80;
const ID_OUT_REPORT_HAPTIC_PULSE: u8 = 0x81;
const ID_OUT_REPORT_HAPTIC_COMMAND: u8 = 0x82;
const ID_OUT_REPORT_HAPTIC_LFO_TONE: u8 = 0x83;
const ID_OUT_REPORT_HAPTIC_LOG_SWEEP: u8 = 0x84;
const ID_OUT_REPORT_HAPTIC_SCRIPT: u8 = 0x85;

const HAPTIC_CMD_OFF: u8 = 0;
const HAPTIC_CMD_CLICK: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    All,
    Rumble,
    Pulse,
    Command,
    Tone,
    Sweep,
    Script,
}

#[derive(Debug, Clone, Copy)]
enum Side {
    Left,
    Right,
    Both,
}

#[derive(Debug)]
struct Config {
    pid: u16,
    interface: Option<i32>,
    path: Option<String>,
    mode: Mode,
    side: Side,
    rumble_duration_ms: u64,
    interval_ms: u64,
    pause_ms: u64,
    low: u16,
    high: u16,
    on_us: u16,
    off_us: u16,
    repeat_count: u16,
    gain_db: i16,
    command: u8,
    frequency: u16,
    effect_ms: u16,
    lfo_freq: u16,
    lfo_depth: u8,
    start_freq: u16,
    end_freq: u16,
    script_id: u8,
    no_stop: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pid: DEFAULT_PID,
            interface: Some(DEFAULT_INTERFACE),
            path: None,
            mode: Mode::Rumble,
            side: Side::Both,
            rumble_duration_ms: DEFAULT_RUMBLE_DURATION_MS,
            interval_ms: DEFAULT_RUMBLE_INTERVAL_MS,
            pause_ms: DEFAULT_PAUSE_MS,
            low: 0x8000,
            high: 0x8000,
            on_us: 2500,
            off_us: 2500,
            repeat_count: 3,
            gain_db: 0,
            command: HAPTIC_CMD_CLICK,
            frequency: 180,
            effect_ms: DEFAULT_EFFECT_MS,
            lfo_freq: 0,
            lfo_depth: 0,
            start_freq: 90,
            end_freq: 260,
            script_id: 0,
            no_stop: false,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;

    let api = HidApi::new()?;
    let mut candidates = api
        .device_list()
        .filter(|device| device.vendor_id() == VALVE_VENDOR_ID && device.product_id() == config.pid)
        .filter(|device| {
            config
                .interface
                .is_none_or(|wanted| device.interface_number() == wanted)
        })
        .filter(|device| {
            config
                .path
                .as_ref()
                .is_none_or(|wanted| device.path().to_string_lossy().as_ref() == wanted)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|device| {
        (
            std::cmp::Reverse(target_score(device)),
            device.path().to_string_lossy().into_owned(),
            device.usage_page(),
            device.usage(),
        )
    });

    print_candidates(&candidates);

    let Some(target) = candidates.first() else {
        return Err("no matching Valve Triton/Proteus HID device".into());
    };

    let path = CString::new(target.path().to_bytes())?;
    let device = api.open_path(path.as_c_str())?;

    println!(
        "Target: PID=0x{:04X} interface={} usage_page=0x{:04X} usage=0x{:04X} path={}",
        target.product_id(),
        target.interface_number(),
        target.usage_page(),
        target.usage(),
        target.path().to_string_lossy()
    );

    run_feedback(&device, &config)?;
    Ok(())
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pid" => config.pid = parse_u16(&value(&arg, args.next())?)?,
            "--interface" => config.interface = Some(value(&arg, args.next())?.parse()?),
            "--any-interface" => config.interface = None,
            "--path" => config.path = Some(value(&arg, args.next())?),
            "--mode" => config.mode = parse_mode(&value(&arg, args.next())?)?,
            "--side" => config.side = parse_side(&value(&arg, args.next())?)?,
            "--duration-ms" => {
                config.rumble_duration_ms = parse_u64_nonzero(&arg, args.next())?;
            }
            "--interval-ms" => {
                config.interval_ms = parse_u64_nonzero(&arg, args.next())?;
            }
            "--pause-ms" => {
                config.pause_ms = parse_u64(&value(&arg, args.next())?)?;
            }
            "--low" => config.low = parse_u16(&value(&arg, args.next())?)?,
            "--high" => config.high = parse_u16(&value(&arg, args.next())?)?,
            "--on-us" => config.on_us = parse_u16(&value(&arg, args.next())?)?,
            "--off-us" => config.off_us = parse_u16(&value(&arg, args.next())?)?,
            "--repeat" => config.repeat_count = parse_u16(&value(&arg, args.next())?)?,
            "--gain-db" => config.gain_db = parse_i16(&value(&arg, args.next())?)?,
            "--command" => config.command = parse_haptic_command(&value(&arg, args.next())?)?,
            "--frequency" => config.frequency = parse_u16(&value(&arg, args.next())?)?,
            "--effect-ms" => config.effect_ms = parse_u16(&value(&arg, args.next())?)?,
            "--lfo-freq" => config.lfo_freq = parse_u16(&value(&arg, args.next())?)?,
            "--lfo-depth" => config.lfo_depth = parse_u8(&value(&arg, args.next())?)?,
            "--start-freq" => config.start_freq = parse_u16(&value(&arg, args.next())?)?,
            "--end-freq" => config.end_freq = parse_u16(&value(&arg, args.next())?)?,
            "--script-id" => config.script_id = parse_u8(&value(&arg, args.next())?)?,
            "--no-stop" => config.no_stop = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }

    Ok(config)
}

fn print_help() {
    println!(
        r#"triton_feedback

Send Steam Triton/Proteus haptic output reports to the host HID device.
Default target is Valve VID 0x28DE, PID 0x1304, interface 2.

Usage:
  cargo run -p crosspuck-cli --example triton_feedback -- [options]

Options:
  --pid <hex|dec>        Product ID override for non-default Valve devices (default: 0x1304)
  --interface <number>   HID interface filter (default: 2)
  --any-interface        Do not filter by HID interface
  --path <path>          Open a specific HID path
  --mode <mode>          all|rumble|pulse|command|tone|sweep|script (default: rumble)
  --side <side>          left|right|both (default: both)
  --duration-ms <ms>     Rumble resend duration (default: 300)
  --interval-ms <ms>     Rumble resend interval (default: 40)
  --pause-ms <ms>        Pause between modes when --mode all (default: 180)
  --low <hex|dec>        Rumble left/low speed, u16 (default: 0x8000)
  --high <hex|dec>       Rumble right/high speed, u16 (default: 0x8000)
  --on-us <us>           Pulse on duration, u16 (default: 2500)
  --off-us <us>          Pulse off duration, u16 (default: 2500)
  --repeat <n>           Pulse repeat count, u16 (default: 3)
  --gain-db <n>          Haptic gain in dB-style units (default: 0)
  --command <cmd>        off|tick|click|tone|rumble|noise|script|sweep|hex|dec (default: click)
  --frequency <hz>       Tone frequency, u16 (default: 180)
  --effect-ms <ms>       Tone/sweep duration, u16 (default: 120)
  --lfo-freq <hz>        Tone LFO frequency, u16 (default: 0)
  --lfo-depth <percent>  Tone LFO depth, u8 (default: 0)
  --start-freq <hz>      Sweep start frequency, u16 (default: 90)
  --end-freq <hz>        Sweep end frequency, u16 (default: 260)
  --script-id <id>       Script id for --mode script (default: 0)
  --no-stop              Do not send final rumble zero/off command
  -h, --help             Show this help

Examples:
  cargo run -p crosspuck-cli --example triton_feedback -- --mode all
  cargo run -p crosspuck-cli --example triton_feedback -- --mode tone --side left --frequency 220 --effect-ms 150 --gain-db -6
  cargo run -p crosspuck-cli --example triton_feedback -- --mode pulse --side right --on-us 1800 --off-us 3000 --repeat 4
  cargo run -p crosspuck-cli --example triton_feedback -- --duration-ms 300 --low 0xffff --high 0xffff
"#
    );
}

fn print_candidates(candidates: &[&DeviceInfo]) {
    if candidates.is_empty() {
        println!("No matching Valve HID devices.");
        return;
    }

    println!("Matching Valve HID devices: {}", candidates.len());
    for (index, device) in candidates.iter().enumerate() {
        println!(
            "[{index}] PID=0x{:04X} interface={} usage_page=0x{:04X} usage=0x{:04X} path={} product={:?} serial={:?}",
            device.product_id(),
            device.interface_number(),
            device.usage_page(),
            device.usage(),
            device.path().to_string_lossy(),
            device.product_string(),
            device.serial_number()
        );
    }
}

fn target_score(device: &DeviceInfo) -> u8 {
    match (device.usage_page(), device.usage()) {
        (0x0001, 0x0002) => 3,
        (0xFF00, 0x0001) => 2,
        (0x0001, 0x0001) => 1,
        _ => 0,
    }
}

fn run_feedback(device: &HidDevice, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let modes = if config.mode == Mode::All {
        vec![
            Mode::Rumble,
            Mode::Pulse,
            Mode::Command,
            Mode::Tone,
            Mode::Sweep,
            Mode::Script,
        ]
    } else {
        vec![config.mode]
    };

    for (index, mode) in modes.iter().enumerate() {
        run_mode(device, *mode, config)?;
        if config.mode == Mode::All && index + 1 < modes.len() && config.pause_ms > 0 {
            sleep(Duration::from_millis(config.pause_ms));
        }
    }

    if !config.no_stop {
        send_stop(device, config.side)?;
    }

    println!("Done.");
    Ok(())
}

fn run_mode(
    device: &HidDevice,
    mode: Mode,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    match mode {
        Mode::All => unreachable!("Mode::All is expanded before run_mode"),
        Mode::Rumble => play_rumble(device, config),
        Mode::Pulse => {
            let packet = triton_pulse_packet(
                config.side,
                config.on_us,
                config.off_us,
                config.repeat_count,
                config.gain_db,
            );
            write_named_packet(device, "PULSE", &packet)
        }
        Mode::Command => {
            let packet = triton_command_packet(config.side, config.command, gain_i8(config)?);
            write_named_packet(device, "COMMAND", &packet)
        }
        Mode::Tone => {
            let packet = triton_tone_packet(
                config.side,
                gain_i8(config)?,
                config.frequency,
                config.effect_ms,
                config.lfo_freq,
                config.lfo_depth,
            );
            write_named_packet(device, "TONE", &packet)
        }
        Mode::Sweep => {
            let packet = triton_sweep_packet(
                config.side,
                gain_i8(config)?,
                config.effect_ms,
                config.start_freq,
                config.end_freq,
            );
            write_named_packet(device, "SWEEP", &packet)
        }
        Mode::Script => {
            let packet = triton_script_packet(config.side, config.script_id, gain_i8(config)?);
            write_named_packet(device, "SCRIPT", &packet)
        }
    }
}

fn play_rumble(device: &HidDevice, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let rumble = triton_rumble_packet(config.low, config.high);
    let deadline = Instant::now() + Duration::from_millis(config.rumble_duration_ms);
    let interval = Duration::from_millis(config.interval_ms);
    let mut writes = 0_u64;

    println!(
        "RUMBLE {}ms every {}ms low=0x{:04X} high=0x{:04X} packet={}",
        config.rumble_duration_ms,
        config.interval_ms,
        config.low,
        config.high,
        hex(&rumble)
    );

    loop {
        write_packet(device, &rumble)?;
        writes += 1;

        let now = Instant::now();
        if now >= deadline {
            break;
        }

        sleep((deadline - now).min(interval));
    }

    println!("RUMBLE writes={writes}");
    Ok(())
}

fn send_stop(device: &HidDevice, side: Side) -> Result<(), Box<dyn std::error::Error>> {
    let rumble_stop = triton_rumble_packet(0, 0);
    let command_off = triton_command_packet(side, HAPTIC_CMD_OFF, 0);
    write_named_packet(device, "STOP rumble", &rumble_stop)?;
    write_named_packet(device, "STOP command", &command_off)
}

fn triton_rumble_packet(low: u16, high: u16) -> [u8; 10] {
    let mut packet = [0_u8; 10];
    packet[0] = ID_OUT_REPORT_HAPTIC_RUMBLE;
    packet[1] = 0; // type
    packet[2..4].copy_from_slice(&0_u16.to_le_bytes()); // intensity
    packet[4..6].copy_from_slice(&low.to_le_bytes());
    packet[6] = 0; // left gain
    packet[7..9].copy_from_slice(&high.to_le_bytes());
    packet[9] = 0; // right gain
    packet
}

fn triton_pulse_packet(
    side: Side,
    on_us: u16,
    off_us: u16,
    repeat_count: u16,
    gain_db: i16,
) -> [u8; 10] {
    let mut packet = [0_u8; 10];
    packet[0] = ID_OUT_REPORT_HAPTIC_PULSE;
    packet[1] = side.code();
    packet[2..4].copy_from_slice(&on_us.to_le_bytes());
    packet[4..6].copy_from_slice(&off_us.to_le_bytes());
    packet[6..8].copy_from_slice(&repeat_count.to_le_bytes());
    packet[8..10].copy_from_slice(&gain_db.to_le_bytes());
    packet
}

fn triton_command_packet(side: Side, command: u8, gain_db: i8) -> [u8; 4] {
    [
        ID_OUT_REPORT_HAPTIC_COMMAND,
        side.code(),
        command,
        gain_db as u8,
    ]
}

fn triton_tone_packet(
    side: Side,
    gain_db: i8,
    frequency: u16,
    duration_ms: u16,
    lfo_freq: u16,
    lfo_depth: u8,
) -> [u8; 10] {
    let mut packet = [0_u8; 10];
    packet[0] = ID_OUT_REPORT_HAPTIC_LFO_TONE;
    packet[1] = side.code();
    packet[2] = gain_db as u8;
    packet[3..5].copy_from_slice(&frequency.to_le_bytes());
    packet[5..7].copy_from_slice(&duration_ms.to_le_bytes());
    packet[7..9].copy_from_slice(&lfo_freq.to_le_bytes());
    packet[9] = lfo_depth;
    packet
}

fn triton_sweep_packet(
    side: Side,
    gain_db: i8,
    duration_ms: u16,
    start_freq: u16,
    end_freq: u16,
) -> [u8; 9] {
    let mut packet = [0_u8; 9];
    packet[0] = ID_OUT_REPORT_HAPTIC_LOG_SWEEP;
    packet[1] = side.code();
    packet[2] = gain_db as u8;
    packet[3..5].copy_from_slice(&duration_ms.to_le_bytes());
    packet[5..7].copy_from_slice(&start_freq.to_le_bytes());
    packet[7..9].copy_from_slice(&end_freq.to_le_bytes());
    packet
}

fn triton_script_packet(side: Side, script_id: u8, gain_db: i8) -> [u8; 4] {
    [
        ID_OUT_REPORT_HAPTIC_SCRIPT,
        side.code(),
        script_id,
        gain_db as u8,
    ]
}

fn write_named_packet(
    device: &HidDevice,
    name: &str,
    packet: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{name} packet={}", hex(packet));
    write_packet(device, packet)
}

fn write_packet(device: &HidDevice, packet: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let written = device.write(packet)?;
    if written != packet.len() {
        return Err(format!("short HID write: wrote {written} of {} bytes", packet.len()).into());
    }
    Ok(())
}

impl Side {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0x01,
            Self::Right => 0x02,
            Self::Both => 0x03,
        }
    }
}

fn gain_i8(config: &Config) -> Result<i8, Box<dyn std::error::Error>> {
    Ok(i8::try_from(config.gain_db).map_err(|_| {
        format!(
            "--gain-db must fit in signed 8-bit range for this mode: {}",
            config.gain_db
        )
    })?)
}

fn parse_mode(raw: &str) -> Result<Mode, Box<dyn std::error::Error>> {
    match raw.to_ascii_lowercase().as_str() {
        "all" => Ok(Mode::All),
        "rumble" => Ok(Mode::Rumble),
        "pulse" => Ok(Mode::Pulse),
        "command" => Ok(Mode::Command),
        "tone" | "lfo-tone" | "lfo_tone" => Ok(Mode::Tone),
        "sweep" | "log-sweep" | "log_sweep" => Ok(Mode::Sweep),
        "script" => Ok(Mode::Script),
        _ => Err(format!("unknown mode: {raw}").into()),
    }
}

fn parse_side(raw: &str) -> Result<Side, Box<dyn std::error::Error>> {
    match raw.to_ascii_lowercase().as_str() {
        "left" | "l" => Ok(Side::Left),
        "right" | "r" => Ok(Side::Right),
        "both" | "b" => Ok(Side::Both),
        _ => Err(format!("unknown side: {raw}").into()),
    }
}

fn parse_haptic_command(raw: &str) -> Result<u8, Box<dyn std::error::Error>> {
    match raw.to_ascii_lowercase().as_str() {
        "off" => Ok(0),
        "tick" => Ok(1),
        "click" => Ok(2),
        "tone" => Ok(3),
        "rumble" => Ok(4),
        "noise" => Ok(5),
        "script" => Ok(6),
        "sweep" => Ok(7),
        _ => parse_u8(raw),
    }
}

fn value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn parse_i16(raw: &str) -> Result<i16, Box<dyn std::error::Error>> {
    Ok(parse_i64(raw)?.try_into()?)
}

fn parse_u16(raw: &str) -> Result<u16, Box<dyn std::error::Error>> {
    Ok(parse_u64(raw)?.try_into()?)
}

fn parse_u8(raw: &str) -> Result<u8, Box<dyn std::error::Error>> {
    Ok(parse_u64(raw)?.try_into()?)
}

fn parse_u64_nonzero(flag: &str, raw: Option<String>) -> Result<u64, Box<dyn std::error::Error>> {
    let value = parse_u64(&value(flag, raw)?)?;
    if value == 0 {
        return Err(format!("{flag} must be greater than 0").into());
    }
    Ok(value)
}

fn parse_i64(raw: &str) -> Result<i64, Box<dyn std::error::Error>> {
    if let Some(hex) = raw.strip_prefix("-0x").or_else(|| raw.strip_prefix("-0X")) {
        Ok(-i64::from_str_radix(hex, 16)?)
    } else if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Ok(i64::from_str_radix(hex, 16)?)
    } else {
        Ok(raw.parse()?)
    }
}

fn parse_u64(raw: &str) -> Result<u64, Box<dyn std::error::Error>> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Ok(u64::from_str_radix(hex, 16)?)
    } else {
        Ok(raw.parse()?)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
