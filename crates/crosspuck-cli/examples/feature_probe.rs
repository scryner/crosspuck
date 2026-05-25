use hidapi::HidApi;
use std::env;
use std::ffi::CString;
use std::thread::sleep;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pid = 0x1304_u16;
    let mut interface = Some(2_i32);
    let mut send = Vec::<u8>::new();
    let mut get_report_id = 0x02_u8;
    let mut get_len = 64_usize;
    let mut send_len = 64_usize;
    let mut repeat = 1_usize;
    let mut delay_ms = 50_u64;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pid" => pid = parse_u16(&value(&arg, args.next())?)?,
            "--interface" => interface = Some(value(&arg, args.next())?.parse()?),
            "--any-interface" => interface = None,
            "--send" => send = parse_hex(&value(&arg, args.next())?)?,
            "--send-len" => send_len = value(&arg, args.next())?.parse()?,
            "--get-report-id" => get_report_id = parse_u8(&value(&arg, args.next())?)?,
            "--get-len" => get_len = value(&arg, args.next())?.parse()?,
            "--repeat" => repeat = value(&arg, args.next())?.parse()?,
            "--delay-ms" => delay_ms = value(&arg, args.next())?.parse()?,
            "-h" | "--help" => {
                println!(
                    "Usage: cargo run -p crosspuck-cli --example feature_probe -- --pid 0x1304 --interface 2 --send \"02 A3 00\" --get-report-id 0x02 --repeat 8"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }

    let api = HidApi::new()?;
    let mut candidates = api
        .device_list()
        .filter(|device| device.vendor_id() == 0x28DE && device.product_id() == pid)
        .filter(|device| interface.is_none_or(|wanted| device.interface_number() == wanted))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|device| device.path().to_string_lossy().into_owned());

    for (index, device) in candidates.iter().enumerate() {
        println!(
            "[{index}] VID=0x{:04X} PID=0x{:04X} interface={} usage_page=0x{:04X} usage=0x{:04X} path={} product={:?} serial={:?}",
            device.vendor_id(),
            device.product_id(),
            device.interface_number(),
            device.usage_page(),
            device.usage(),
            device.path().to_string_lossy(),
            device.product_string(),
            device.serial_number()
        );
    }

    let Some(target) = candidates.first() else {
        return Err("no matching Valve HID device".into());
    };
    let path = CString::new(target.path().to_bytes())?;
    let device = api.open_path(path.as_c_str())?;

    if !send.is_empty() {
        if send.len() < send_len {
            send.resize(send_len, 0);
        }
        device.send_feature_report(&send)?;
        println!("SEND {} bytes: {}", send.len(), hex(&send));
    }

    for index in 0..repeat {
        sleep(Duration::from_millis(delay_ms));
        let mut buffer = vec![0_u8; get_len];
        buffer[0] = get_report_id;
        match device.get_feature_report(&mut buffer) {
            Ok(read) => println!(
                "GET #{:<3} {} bytes report_id=0x{:02X}: {}",
                index + 1,
                read,
                get_report_id,
                hex(&buffer[..read.min(buffer.len())])
            ),
            Err(error) => println!("GET #{:<3} error: {}", index + 1, error),
        }
    }

    Ok(())
}

fn value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn parse_u16(raw: &str) -> Result<u16, Box<dyn std::error::Error>> {
    Ok(parse_u64(raw)?.try_into()?)
}

fn parse_u8(raw: &str) -> Result<u8, Box<dyn std::error::Error>> {
    Ok(parse_u64(raw)?.try_into()?)
}

fn parse_u64(raw: &str) -> Result<u64, Box<dyn std::error::Error>> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Ok(u64::from_str_radix(hex, 16)?)
    } else {
        Ok(raw.parse()?)
    }
}

fn parse_hex(raw: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    raw.split_whitespace()
        .map(|word| Ok(u8::from_str_radix(word.trim_start_matches("0x"), 16)?))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
