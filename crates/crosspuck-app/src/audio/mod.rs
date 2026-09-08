//! Audio routing has its own process and lifetime, independent of the HID
//! bridge and selected Steam bottle. No PCM crosses this control channel.
use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    Starting,
    Stopping,
    Waiting(String),
    Connecting(String),
    Routing { output: String, processes: usize },
    Retrying,
    AnotherInstance,
    Unsupported,
    MissingHelper,
}

impl Status {
    pub fn title(&self) -> String {
        let text = match self {
            Self::Off => "Automatic switching off".into(),
            Self::Starting => "Starting…".into(),
            Self::Stopping => "Restoring original output…".into(),
            Self::Waiting(output) if !output.is_empty() => {
                format!("Following {output}")
            }
            Self::Waiting(_) => "Waiting for an output device".into(),
            Self::Connecting(output) => format!("Connecting to {output} — check audio permission"),
            Self::Routing { output, .. } => format!("Playing through {output}"),
            Self::Retrying => "Unavailable — retrying; check audio permission".into(),
            Self::AnotherInstance => "Handled by another CrossPuck".into(),
            Self::Unsupported => "Requires macOS 14.2 or later".into(),
            Self::MissingHelper => "Helper unavailable — rebuild or reinstall CrossPuck".into(),
        };
        format!("Audio: {text}")
    }
}

struct Shared {
    enabled: AtomicBool,
    revision: AtomicU64,
    stopping: AtomicBool,
    supported: bool,
    status: Mutex<Status>,
    wake: Mutex<Option<Thread>>,
}

// The worker owns Shared, never ServiceOwner. Dropping the last public handle
// can therefore stop and join it instead of leaving a self-owned worker alive.
struct ServiceOwner {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl ServiceOwner {
    fn shutdown(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.notify();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ServiceOwner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
pub struct AudioService(Arc<ServiceOwner>);

impl AudioService {
    pub fn start(enabled: bool) -> Self {
        let supported = NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(
            NSOperatingSystemVersion {
                majorVersion: 14,
                minorVersion: 2,
                patchVersion: 0,
            },
        );
        Self::with_support(enabled, supported)
    }

    fn with_support(enabled: bool, supported: bool) -> Self {
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(enabled),
            revision: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            supported,
            status: Mutex::new(if !supported {
                Status::Unsupported
            } else if enabled {
                Status::Starting
            } else {
                Status::Off
            }),
            wake: Mutex::new(None),
        });
        let owner = Arc::new(ServiceOwner {
            shared: Arc::clone(&shared),
            worker: Mutex::new(None),
        });
        if supported {
            let worker_state = Arc::clone(&shared);
            match thread::Builder::new()
                .name("crosspuck-audio".into())
                .spawn(move || supervise(worker_state))
            {
                Ok(worker) => *owner.worker.lock().unwrap() = Some(worker),
                Err(error) => {
                    log::error!("Could not start audio supervisor: {error}");
                    shared.set_status(Status::Retrying);
                }
            }
        }
        Self(owner)
    }

    pub fn enabled(&self) -> bool {
        self.0.shared.enabled.load(Ordering::Acquire)
    }
    pub fn supported(&self) -> bool {
        self.0.shared.supported
    }
    pub fn status(&self) -> Status {
        self.0.shared.status.lock().unwrap().clone()
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.0.shared.enabled.store(enabled, Ordering::Release);
        self.0.shared.revision.fetch_add(1, Ordering::Release);
        self.0.shared.notify();
    }

    pub fn shutdown(&self) {
        self.0.shutdown();
    }
}

impl Shared {
    fn notify(&self) {
        if let Some(worker) = self.wake.lock().unwrap().as_ref() {
            // unpark keeps a token if notification precedes park: no lost wake
            // between checking enabled/stopping/the event queue and sleeping.
            worker.unpark();
        }
    }

    fn set_status(&self, next: Status) {
        let mut current = self.status.lock().unwrap();
        if *current != next {
            log::info!("{}", next.title());
            *current = next;
        }
    }
}

#[derive(Debug, Deserialize)]
struct Event {
    event: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    output: String,
    #[serde(default)]
    processes: usize,
}

impl Event {
    fn status(self) -> Option<Status> {
        if self.event != "state" {
            return None;
        }
        match self.state.as_str() {
            "waiting" => Some(Status::Waiting(self.output)),
            "connecting" => Some(Status::Connecting(self.output)),
            "routing" => Some(Status::Routing {
                output: self.output,
                processes: self.processes,
            }),
            "retrying" => Some(Status::Retrying),
            "another_instance" => Some(Status::AnotherInstance),
            _ => None,
        }
    }
}

fn helper_path() -> std::io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    Ok(executable
        .parent()
        .unwrap_or(Path::new("."))
        .join("CrossPuckAudio"))
}

fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs((1u64 << failures.min(5)).min(30))
}

fn stop_child(child: &mut Child, grace: Duration) {
    // Closing the sole writer also protects against abrupt parent termination.
    // The native helper independently enforces its own 3-second exit deadline.
    drop(child.stdin.take());
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                log::warn!("Audio helper wait failed: {error}");
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn supervise(shared: Arc<Shared>) {
    *shared.wake.lock().unwrap() = Some(thread::current());
    let path = helper_path().ok();
    let mut failures = 0;
    let mut revision = shared.revision.load(Ordering::Acquire);
    let mut next_start = Instant::now();
    while !shared.stopping.load(Ordering::Acquire) {
        let current_revision = shared.revision.load(Ordering::Acquire);
        if revision != current_revision {
            revision = current_revision;
            failures = 0;
            next_start = Instant::now();
        }
        if !shared.enabled.load(Ordering::Acquire) {
            shared.set_status(Status::Off);
            thread::park();
            continue;
        }
        if Instant::now() < next_start {
            thread::park_timeout(next_start.saturating_duration_since(Instant::now()));
            continue;
        }
        let Some(path) = path.as_ref().filter(|path| path.is_file()) else {
            shared.set_status(Status::MissingHelper);
            next_start = Instant::now() + Duration::from_secs(1);
            continue;
        };
        shared.set_status(Status::Starting);
        let mut child = match Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                log::error!("Could not launch audio helper: {error}");
                shared.set_status(Status::Retrying);
                next_start = Instant::now() + retry_delay(failures);
                failures = failures.saturating_add(1);
                continue;
            }
        };
        log::debug!("Audio helper started: pid={}", child.id());
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(32);
        let reader_state = Arc::clone(&shared);
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                match sender.try_send(line) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => (),
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
                reader_state.notify();
            }
            // EOF also wakes the supervisor when the child crashes without
            // emitting a final event. Drop the sender before notification.
            drop(sender);
            reader_state.notify();
        });
        let started = Instant::now();
        let mut last_event = started;
        let mut allowance = Duration::from_secs(90);
        let mut healthy_since = None;
        loop {
            if shared.stopping.load(Ordering::Acquire) || !shared.enabled.load(Ordering::Acquire) {
                shared.set_status(Status::Stopping);
                break;
            }
            let disconnected = loop {
                let line = match receiver.try_recv() {
                    Ok(line) => line,
                    Err(mpsc::TryRecvError::Empty) => break false,
                    Err(mpsc::TryRecvError::Disconnected) => break true,
                };
                let Ok(event) = serde_json::from_str::<Event>(&line) else {
                    continue;
                };
                last_event = Instant::now();
                if event.event == "error" {
                    log::warn!("Audio routing: {line}");
                } else if event.event != "heartbeat" {
                    log::debug!("Audio routing: {line}");
                }
                if let Some(status) = event.status() {
                    if matches!(status, Status::Waiting(_) | Status::Routing { .. }) {
                        healthy_since.get_or_insert_with(Instant::now);
                    } else {
                        healthy_since = None;
                    }
                    allowance = if matches!(status, Status::Connecting(_)) {
                        Duration::from_secs(90)
                    } else {
                        Duration::from_secs(10)
                    };
                    shared.set_status(status);
                }
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    log::debug!("Audio helper exited: {status}");
                    if status.code() == Some(11) {
                        shared.set_status(Status::AnotherInstance);
                    } else {
                        shared.set_status(Status::Retrying);
                    }
                    break;
                }
                Err(error) => {
                    log::warn!("Audio helper failed: {error}");
                    shared.set_status(Status::Retrying);
                    break;
                }
                Ok(None) => (),
            }
            // EOF can precede process exit by a few instructions. Reap it via
            // stop_child rather than parking until the heartbeat deadline.
            if disconnected {
                break;
            }
            if last_event.elapsed() >= allowance {
                log::warn!(
                    "Audio helper stopped responding; restoring original output and restarting"
                );
                shared.set_status(Status::Retrying);
                break;
            }
            thread::park_timeout(allowance.saturating_sub(last_event.elapsed()));
        }
        stop_child(&mut child, Duration::from_secs(4));
        if shared.enabled.load(Ordering::Acquire) && !shared.stopping.load(Ordering::Acquire) {
            // stdout EOF may arrive just before try_wait can observe the exit.
            // Classify after reaping as well, preserving the lease-conflict UI.
            shared.set_status(match child.try_wait() {
                Ok(Some(status)) if status.code() == Some(11) => Status::AnotherInstance,
                _ => Status::Retrying,
            });
        }
        let _ = reader.join();
        // Preserve final cleanup diagnostics even when disable/quit broke out
        // before the helper emitted them. Do not reapply stale UI states.
        for line in receiver.try_iter() {
            log::debug!("Audio routing shutdown: {line}");
        }
        if healthy_since.is_some_and(|since| since.elapsed() >= Duration::from_secs(60)) {
            failures = 0;
        }
        next_start = Instant::now() + retry_delay(failures);
        failures = failures.saturating_add(1);
    }
    shared.set_status(Status::Off);
    *shared.wake.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_last_handle_stops_parked_worker_and_releases_state() {
        let service = AudioService::with_support(false, true);
        let shared = Arc::downgrade(&service.0.shared);
        let clone = service.clone();
        drop(service);
        assert!(shared.upgrade().is_some());
        let start = Instant::now();
        drop(clone);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(shared.upgrade().is_none());
    }

    #[test]
    fn shutdown_is_idempotent_and_wakes_disabled_worker() {
        let service = AudioService::with_support(false, true);
        service.shutdown();
        service.shutdown();
        assert_eq!(service.status(), Status::Off);
        assert!(service.0.worker.lock().unwrap().is_none());
    }

    #[test]
    fn helper_states_distinguish_native_output_and_active_routing() {
        let event: Event = serde_json::from_str(
            r#"{"event":"state","state":"routing","output":"AirPods Max","processes":2}"#,
        )
        .unwrap();
        assert_eq!(
            event.status(),
            Some(Status::Routing {
                output: "AirPods Max".into(),
                processes: 2
            })
        );
        let event: Event = serde_json::from_str(
            r#"{"event":"state","state":"waiting","output":"Studio Display"}"#,
        )
        .unwrap();
        assert_eq!(
            event.status(),
            Some(Status::Waiting("Studio Display".into()))
        );
        let event: Event = serde_json::from_str(r#"{"event":"heartbeat","frames":0}"#).unwrap();
        assert_eq!(event.status(), None);
    }

    #[test]
    fn repeated_failures_have_bounded_backoff() {
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(4));
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn closing_owner_pipe_stops_cooperative_child() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        stop_child(&mut child, Duration::from_secs(1));
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn unresponsive_child_is_killed_after_grace_period() {
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        stop_child(&mut child, Duration::from_millis(100));
        assert!(!child.wait().unwrap().success());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
