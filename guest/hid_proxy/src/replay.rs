use serde_json::Value;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayPacket {
    pub seq: Option<u64>,
    pub elapsed: Duration,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayScript {
    packets: Vec<ReplayPacket>,
}

impl ReplayScript {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ReplayError> {
        let source = fs::read_to_string(path.as_ref()).map_err(ReplayError::Io)?;
        Self::from_jsonl(&source)
    }

    pub fn from_jsonl(source: &str) -> Result<Self, ReplayError> {
        let mut packets = Vec::new();
        let mut last_elapsed = None;

        for (index, raw_line) in source.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }

            let value: Value = serde_json::from_str(line).map_err(|source| ReplayError::Json {
                line: line_number,
                source,
            })?;

            if value.get("type").and_then(Value::as_str) != Some("packet") {
                continue;
            }

            let elapsed_us = value.get("elapsed_us").and_then(Value::as_u64).ok_or(
                ReplayError::MissingField {
                    line: line_number,
                    field: "elapsed_us",
                },
            )?;

            if last_elapsed.is_some_and(|last| elapsed_us < last) {
                return Err(ReplayError::ElapsedDecreased {
                    line: line_number,
                    previous_us: last_elapsed.unwrap(),
                    current_us: elapsed_us,
                });
            }
            last_elapsed = Some(elapsed_us);

            let expected_len = value.get("bytes_read").and_then(Value::as_u64).ok_or(
                ReplayError::MissingField {
                    line: line_number,
                    field: "bytes_read",
                },
            )? as usize;

            let hex =
                value
                    .get("hex")
                    .and_then(Value::as_str)
                    .ok_or(ReplayError::MissingField {
                        line: line_number,
                        field: "hex",
                    })?;
            let bytes = parse_hex_bytes(hex).map_err(|message| ReplayError::InvalidHex {
                line: line_number,
                message,
            })?;

            if bytes.len() != expected_len {
                return Err(ReplayError::LengthMismatch {
                    line: line_number,
                    expected: expected_len,
                    actual: bytes.len(),
                });
            }

            if bytes.is_empty() {
                return Err(ReplayError::EmptyPacket { line: line_number });
            }

            packets.push(ReplayPacket {
                seq: value.get("seq").and_then(Value::as_u64),
                elapsed: Duration::from_micros(elapsed_us),
                bytes,
            });
        }

        if packets.is_empty() {
            return Err(ReplayError::NoPackets);
        }

        Ok(Self { packets })
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn packets(&self) -> &[ReplayPacket] {
        &self.packets
    }
}

#[derive(Debug)]
pub struct ReplayPlayer {
    script: ReplayScript,
    startup_delay: Duration,
    created_at: Instant,
    replay_origin: Option<Instant>,
    next_index: usize,
    next_idle_at: Option<Instant>,
    idle_interval: Duration,
}

impl ReplayPlayer {
    pub fn new(script: ReplayScript, startup_delay: Duration) -> Self {
        Self {
            script,
            startup_delay,
            created_at: Instant::now(),
            replay_origin: None,
            next_index: 0,
            next_idle_at: None,
            idle_interval: Duration::from_millis(4),
        }
    }

    pub fn packet_count(&self) -> usize {
        self.script.len()
    }

    pub fn next_index(&self) -> usize {
        self.next_index
    }

    pub fn is_complete(&self) -> bool {
        self.next_index >= self.script.len()
    }

    pub fn read_next_blocking(&mut self, output: &mut [u8]) -> Result<usize, ReplayError> {
        if output.is_empty() {
            return Ok(0);
        }

        self.wait_for_startup_delay();

        if self.replay_origin.is_none() {
            self.replay_origin = Some(Instant::now());
        }

        if self.next_index < self.script.len() {
            let packet = &self.script.packets[self.next_index];
            let due_at = self.replay_origin.unwrap() + packet.elapsed;
            sleep_until(due_at);
            self.next_index += 1;
            return Ok(copy_packet(output, &packet.bytes));
        }

        self.replay_idle_packet(output)
    }

    fn wait_for_startup_delay(&self) {
        sleep_until(self.created_at + self.startup_delay);
    }

    fn replay_idle_packet(&mut self, output: &mut [u8]) -> Result<usize, ReplayError> {
        let Some(last_packet) = self.script.packets.last() else {
            return Err(ReplayError::NoPackets);
        };

        let due_at = self.next_idle_at.unwrap_or_else(Instant::now);
        sleep_until(due_at);
        self.next_idle_at = Some(Instant::now() + self.idle_interval);
        Ok(copy_packet(output, &last_packet.bytes))
    }
}

#[derive(Debug)]
pub enum ReplayError {
    Io(std::io::Error),
    Json {
        line: usize,
        source: serde_json::Error,
    },
    MissingField {
        line: usize,
        field: &'static str,
    },
    InvalidHex {
        line: usize,
        message: String,
    },
    LengthMismatch {
        line: usize,
        expected: usize,
        actual: usize,
    },
    EmptyPacket {
        line: usize,
    },
    ElapsedDecreased {
        line: usize,
        previous_us: u64,
        current_us: u64,
    },
    NoPackets,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(source) => write!(f, "failed to read replay file: {source}"),
            Self::Json { line, source } => write!(f, "line {line}: invalid JSON: {source}"),
            Self::MissingField { line, field } => write!(f, "line {line}: missing field {field}"),
            Self::InvalidHex { line, message } => write!(f, "line {line}: invalid hex: {message}"),
            Self::LengthMismatch {
                line,
                expected,
                actual,
            } => write!(
                f,
                "line {line}: bytes_read={expected}, parsed hex byte count={actual}"
            ),
            Self::EmptyPacket { line } => write!(f, "line {line}: empty packet"),
            Self::ElapsedDecreased {
                line,
                previous_us,
                current_us,
            } => write!(
                f,
                "line {line}: elapsed_us decreased from {previous_us} to {current_us}"
            ),
            Self::NoPackets => write!(f, "replay file does not contain packet records"),
        }
    }
}

impl Error for ReplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            Self::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn parse_hex_bytes(hex: &str) -> Result<Vec<u8>, String> {
    hex.split_whitespace()
        .map(|word| {
            u8::from_str_radix(word, 16).map_err(|_| format!("cannot parse hex byte {word:?}"))
        })
        .collect()
}

fn copy_packet(output: &mut [u8], packet: &[u8]) -> usize {
    let bytes_to_copy = output.len().min(packet.len());
    output[..bytes_to_copy].copy_from_slice(&packet[..bytes_to_copy]);
    bytes_to_copy
}

fn sleep_until(instant: Instant) {
    let now = Instant::now();
    if instant > now {
        thread::sleep(instant - now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
{"schema":"crosspuck.host.capture.v1","type":"metadata"}
{"bytes_read":3,"elapsed_us":0,"hex":"42 01 00","seq":1,"type":"packet"}
{"bytes_read":3,"elapsed_us":10,"hex":"42 02 01","seq":2,"type":"packet"}
{"duration_ms":1,"packets":2,"type":"summary"}
"#;

    #[test]
    fn parses_host_capture_jsonl_packets() {
        let script = ReplayScript::from_jsonl(SAMPLE).unwrap();

        assert_eq!(script.len(), 2);
        assert_eq!(script.packets()[0].seq, Some(1));
        assert_eq!(script.packets()[0].elapsed, Duration::from_micros(0));
        assert_eq!(script.packets()[0].bytes, vec![0x42, 0x01, 0x00]);
        assert_eq!(script.packets()[1].bytes, vec![0x42, 0x02, 0x01]);
    }

    #[test]
    fn replays_packets_then_holds_last_packet() {
        let script = ReplayScript::from_jsonl(SAMPLE).unwrap();
        let mut player = ReplayPlayer::new(script, Duration::ZERO);
        let mut buffer = [0_u8; 8];

        assert_eq!(player.read_next_blocking(&mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], &[0x42, 0x01, 0x00]);
        assert_eq!(player.read_next_blocking(&mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], &[0x42, 0x02, 0x01]);
        assert!(player.is_complete());
        assert_eq!(player.read_next_blocking(&mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], &[0x42, 0x02, 0x01]);
    }

    #[test]
    fn rejects_length_mismatch() {
        let err = ReplayScript::from_jsonl(
            r#"{"bytes_read":2,"elapsed_us":0,"hex":"42","seq":1,"type":"packet"}"#,
        )
        .unwrap_err();

        assert!(matches!(err, ReplayError::LengthMismatch { .. }));
    }
}
