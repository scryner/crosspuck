use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{self, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2_foundation::NSAutoreleasePool;

const PROBE_ENV: &str = "CROSSPUCK_PROBE";
const PROBE_DIR_ENV: &str = "CROSSPUCK_PROBE_DIR";
const PROBE_INTERVAL_MS_ENV: &str = "CROSSPUCK_PROBE_INTERVAL_MS";
const PROBE_VMMAP_INTERVAL_MS_ENV: &str = "CROSSPUCK_PROBE_VMMAP_INTERVAL_MS";
const PROBE_CPU_SECONDS_ENV: &str = "CROSSPUCK_PROBE_CPU_SECONDS";
const PROBE_CPU_HZ_ENV: &str = "CROSSPUCK_PROBE_CPU_HZ";
const PROBE_CALLBACK_POOL_ENV: &str = "CROSSPUCK_PROBE_AUTORELEASE_POOL";

const DEFAULT_PROBE_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_VMMAP_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_CPU_HZ: i32 = 99;

static STARTED: AtomicBool = AtomicBool::new(false);
static CALLBACK_AUTORELEASE_POOL: AtomicBool = AtomicBool::new(false);

static UI_TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static MENU_WILL_OPEN: AtomicU64 = AtomicU64::new(0);
static MENU_REFRESHES: AtomicU64 = AtomicU64::new(0);
static DRIVER_STATUS_CHECKS: AtomicU64 = AtomicU64::new(0);
static CONTROL_FRAMES: AtomicU64 = AtomicU64::new(0);
static INPUT_REPORTS: AtomicU64 = AtomicU64::new(0);
static HID_OPEN_PATH_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static HID_INTERFACE_REOPENS: AtomicU64 = AtomicU64::new(0);
static HID_INTERFACE_REOPEN_OK: AtomicU64 = AtomicU64::new(0);
static HID_IDLE_REOPEN_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static HID_IDLE_REOPEN_OK: AtomicU64 = AtomicU64::new(0);
static HID_ERROR_REOPEN_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static HID_ERROR_REOPEN_OK: AtomicU64 = AtomicU64::new(0);
static HID_MAIN_REFRESHES: AtomicU64 = AtomicU64::new(0);
static HID_MAIN_REFRESH_OK: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct ProbeConfig {
    pid: u32,
    dir: PathBuf,
    interval: Duration,
    vmmap_interval: Option<Duration>,
    cpu_duration: Option<Duration>,
    cpu_hz: i32,
    callback_autorelease_pool: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct CounterSnapshot {
    ui_timer_ticks: u64,
    menu_will_open: u64,
    menu_refreshes: u64,
    driver_status_checks: u64,
    control_frames: u64,
    input_reports: u64,
    hid_open_path_attempts: u64,
    hid_interface_reopens: u64,
    hid_interface_reopen_ok: u64,
    hid_idle_reopen_attempts: u64,
    hid_idle_reopen_ok: u64,
    hid_error_reopen_attempts: u64,
    hid_error_reopen_ok: u64,
    hid_main_refreshes: u64,
    hid_main_refresh_ok: u64,
}

impl CounterSnapshot {
    fn now() -> Self {
        Self {
            ui_timer_ticks: UI_TIMER_TICKS.load(Ordering::Relaxed),
            menu_will_open: MENU_WILL_OPEN.load(Ordering::Relaxed),
            menu_refreshes: MENU_REFRESHES.load(Ordering::Relaxed),
            driver_status_checks: DRIVER_STATUS_CHECKS.load(Ordering::Relaxed),
            control_frames: CONTROL_FRAMES.load(Ordering::Relaxed),
            input_reports: INPUT_REPORTS.load(Ordering::Relaxed),
            hid_open_path_attempts: HID_OPEN_PATH_ATTEMPTS.load(Ordering::Relaxed),
            hid_interface_reopens: HID_INTERFACE_REOPENS.load(Ordering::Relaxed),
            hid_interface_reopen_ok: HID_INTERFACE_REOPEN_OK.load(Ordering::Relaxed),
            hid_idle_reopen_attempts: HID_IDLE_REOPEN_ATTEMPTS.load(Ordering::Relaxed),
            hid_idle_reopen_ok: HID_IDLE_REOPEN_OK.load(Ordering::Relaxed),
            hid_error_reopen_attempts: HID_ERROR_REOPEN_ATTEMPTS.load(Ordering::Relaxed),
            hid_error_reopen_ok: HID_ERROR_REOPEN_OK.load(Ordering::Relaxed),
            hid_main_refreshes: HID_MAIN_REFRESHES.load(Ordering::Relaxed),
            hid_main_refresh_ok: HID_MAIN_REFRESH_OK.load(Ordering::Relaxed),
        }
    }

    fn delta_since(self, previous: Self) -> Self {
        Self {
            ui_timer_ticks: self.ui_timer_ticks.saturating_sub(previous.ui_timer_ticks),
            menu_will_open: self.menu_will_open.saturating_sub(previous.menu_will_open),
            menu_refreshes: self.menu_refreshes.saturating_sub(previous.menu_refreshes),
            driver_status_checks: self
                .driver_status_checks
                .saturating_sub(previous.driver_status_checks),
            control_frames: self.control_frames.saturating_sub(previous.control_frames),
            input_reports: self.input_reports.saturating_sub(previous.input_reports),
            hid_open_path_attempts: self
                .hid_open_path_attempts
                .saturating_sub(previous.hid_open_path_attempts),
            hid_interface_reopens: self
                .hid_interface_reopens
                .saturating_sub(previous.hid_interface_reopens),
            hid_interface_reopen_ok: self
                .hid_interface_reopen_ok
                .saturating_sub(previous.hid_interface_reopen_ok),
            hid_idle_reopen_attempts: self
                .hid_idle_reopen_attempts
                .saturating_sub(previous.hid_idle_reopen_attempts),
            hid_idle_reopen_ok: self
                .hid_idle_reopen_ok
                .saturating_sub(previous.hid_idle_reopen_ok),
            hid_error_reopen_attempts: self
                .hid_error_reopen_attempts
                .saturating_sub(previous.hid_error_reopen_attempts),
            hid_error_reopen_ok: self
                .hid_error_reopen_ok
                .saturating_sub(previous.hid_error_reopen_ok),
            hid_main_refreshes: self
                .hid_main_refreshes
                .saturating_sub(previous.hid_main_refreshes),
            hid_main_refresh_ok: self
                .hid_main_refresh_ok
                .saturating_sub(previous.hid_main_refresh_ok),
        }
    }

    fn format(self) -> String {
        format!(
            "ui_timer={} menu_open={} menu_refresh={} driver_status={} control_frames={} input_reports={} hid_open_path={} hid_interface_reopen={}/{} hid_idle_reopen={}/{} hid_error_reopen={}/{} hid_main_refresh={}/{}",
            self.ui_timer_ticks,
            self.menu_will_open,
            self.menu_refreshes,
            self.driver_status_checks,
            self.control_frames,
            self.input_reports,
            self.hid_open_path_attempts,
            self.hid_interface_reopen_ok,
            self.hid_interface_reopens,
            self.hid_idle_reopen_ok,
            self.hid_idle_reopen_attempts,
            self.hid_error_reopen_ok,
            self.hid_error_reopen_attempts,
            self.hid_main_refresh_ok,
            self.hid_main_refreshes
        )
    }
}

pub(crate) fn start_from_env() {
    let callback_autorelease_pool = env_bool(PROBE_CALLBACK_POOL_ENV);
    CALLBACK_AUTORELEASE_POOL.store(callback_autorelease_pool, Ordering::Relaxed);

    if !env_bool(PROBE_ENV) {
        if callback_autorelease_pool {
            log::info!("CrossPuck probe autorelease pool experiment enabled");
        }
        return;
    }
    if STARTED
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let config = ProbeConfig::from_env();
    if let Err(error) = fs::create_dir_all(&config.dir) {
        log::warn!(
            "CrossPuck probe disabled: failed to create {}: {error}",
            config.dir.display()
        );
        return;
    }

    log::info!(
        "CrossPuck probe enabled: dir={} interval={}ms vmmap_interval={}ms cpu_seconds={} callback_autorelease_pool={}",
        config.dir.display(),
        config.interval.as_millis(),
        config
            .vmmap_interval
            .map(|duration| duration.as_millis().to_string())
            .unwrap_or_else(|| "disabled".to_string()),
        config
            .cpu_duration
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|| "disabled".to_string()),
        config.callback_autorelease_pool
    );

    start_cpu_profile(&config);
    let _ = thread::Builder::new()
        .name("crosspuck-probe".to_string())
        .spawn(move || run_probe_loop(config));
}

pub(crate) fn note_ui_timer_tick() {
    UI_TIMER_TICKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_menu_will_open() {
    MENU_WILL_OPEN.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_menu_refresh() {
    MENU_REFRESHES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_driver_status_check() {
    DRIVER_STATUS_CHECKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_control_frame() {
    CONTROL_FRAMES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_input_report() {
    INPUT_REPORTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_open_path_attempt() {
    HID_OPEN_PATH_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_interface_reopen_attempt() {
    HID_INTERFACE_REOPENS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_interface_reopen_ok() {
    HID_INTERFACE_REOPEN_OK.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_error_reopen_attempt() {
    HID_ERROR_REOPEN_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_error_reopen_ok() {
    HID_ERROR_REOPEN_OK.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_main_refresh_attempt() {
    HID_MAIN_REFRESHES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hid_main_refresh_ok() {
    HID_MAIN_REFRESH_OK.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn with_callback_autorelease_pool<T>(body: impl FnOnce() -> T) -> T {
    if CALLBACK_AUTORELEASE_POOL.load(Ordering::Relaxed) {
        unsafe {
            let _pool = NSAutoreleasePool::new();
            body()
        }
    } else {
        body()
    }
}

impl ProbeConfig {
    fn from_env() -> Self {
        let pid = process::id();
        Self {
            pid,
            dir: env::var_os(PROBE_DIR_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| default_probe_dir(pid)),
            interval: env_duration_ms(PROBE_INTERVAL_MS_ENV).unwrap_or(DEFAULT_PROBE_INTERVAL),
            vmmap_interval: env_duration_ms(PROBE_VMMAP_INTERVAL_MS_ENV)
                .or(Some(DEFAULT_VMMAP_INTERVAL))
                .filter(|duration| !duration.is_zero()),
            cpu_duration: env_duration_secs(PROBE_CPU_SECONDS_ENV)
                .filter(|duration| !duration.is_zero()),
            cpu_hz: env_i32(PROBE_CPU_HZ_ENV).unwrap_or(DEFAULT_CPU_HZ).max(1),
            callback_autorelease_pool: env_bool(PROBE_CALLBACK_POOL_ENV),
        }
    }
}

fn run_probe_loop(config: ProbeConfig) {
    let path = config.dir.join("probe.log");
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "CrossPuck probe disabled: failed to open {}: {error}",
                path.display()
            );
            return;
        }
    };

    let mut previous = CounterSnapshot::now();
    let mut previous_rss = current_rss_kb(config.pid);
    let mut last_vmmap = Instant::now()
        .checked_sub(config.vmmap_interval.unwrap_or(DEFAULT_VMMAP_INTERVAL))
        .unwrap_or_else(Instant::now);

    write_probe_line(
        &mut file,
        format!(
            "probe_start pid={} dir={} interval_ms={} vmmap_interval_ms={} cpu_hz={} profiling_feature={} callback_autorelease_pool={}",
            config.pid,
            config.dir.display(),
            config.interval.as_millis(),
            config
                .vmmap_interval
                .map(|duration| duration.as_millis().to_string())
                .unwrap_or_else(|| "disabled".to_string()),
            config.cpu_hz,
            cfg!(feature = "profiling"),
            config.callback_autorelease_pool
        ),
    );

    loop {
        thread::sleep(config.interval);
        let now = CounterSnapshot::now();
        let delta = now.delta_since(previous);
        previous = now;

        let rss = current_rss_kb(config.pid);
        let rss_delta = match (rss, previous_rss) {
            (Some(current), Some(previous)) => current as i64 - previous as i64,
            _ => 0,
        };
        previous_rss = rss.or(previous_rss);

        write_probe_line(
            &mut file,
            format!(
                "probe_tick rss_kb={} rss_delta_kb={} delta=[{}] total=[{}]",
                rss.map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                rss_delta,
                delta.format(),
                now.format()
            ),
        );

        if config
            .vmmap_interval
            .is_some_and(|interval| last_vmmap.elapsed() >= interval)
        {
            last_vmmap = Instant::now();
            match vmmap_summary(config.pid) {
                Some(summary) => write_probe_line(&mut file, format!("probe_vmmap {summary}")),
                None => write_probe_line(&mut file, "probe_vmmap unavailable".to_string()),
            }
        }
    }
}

#[cfg(feature = "profiling")]
fn start_cpu_profile(config: &ProbeConfig) {
    let Some(duration) = config.cpu_duration else {
        return;
    };
    let pid = config.pid;
    let dir = config.dir.clone();
    let hz = config.cpu_hz;
    let _ = thread::Builder::new()
        .name("crosspuck-pprof".to_string())
        .spawn(move || {
            let output = dir.join(format!("cpu-{pid}-{}s.svg", duration.as_secs()));
            log::info!(
                "CrossPuck CPU profiler started: duration={}s hz={} output={}",
                duration.as_secs(),
                hz,
                output.display()
            );

            let guard = match pprof::ProfilerGuard::new(hz) {
                Ok(guard) => guard,
                Err(error) => {
                    log::warn!("CrossPuck CPU profiler failed to start: {error}");
                    return;
                }
            };
            thread::sleep(duration);

            let report = match guard.report().build() {
                Ok(report) => report,
                Err(error) => {
                    log::warn!("CrossPuck CPU profiler failed to build report: {error}");
                    return;
                }
            };
            let file = match File::create(&output) {
                Ok(file) => file,
                Err(error) => {
                    log::warn!(
                        "CrossPuck CPU profiler failed to create {}: {error}",
                        output.display()
                    );
                    return;
                }
            };
            if let Err(error) = report.flamegraph(file) {
                log::warn!("CrossPuck CPU profiler failed to write flamegraph: {error}");
                return;
            }
            log::info!("CrossPuck CPU profiler wrote {}", output.display());
        });
}

#[cfg(not(feature = "profiling"))]
fn start_cpu_profile(config: &ProbeConfig) {
    if config.cpu_duration.is_some() {
        log::warn!(
            "CrossPuck CPU profiler requested but this binary was built without --features profiling"
        );
    }
}

fn write_probe_line(file: &mut File, line: String) {
    log::info!("CrossPuck {line}");
    let _ = writeln!(file, "{} {line}", unix_timestamp_millis());
    let _ = file.flush();
}

fn current_rss_kb(pid: u32) -> Option<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

fn vmmap_summary(pid: u32) -> Option<String> {
    let output = Command::new("/usr/bin/vmmap")
        .args(["-summary", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("Physical footprint:")
                || line.starts_with("MALLOC_SMALL")
                || line.starts_with("MALLOC_TINY")
                || line.starts_with("MALLOC metadata")
                || line.starts_with("DefaultMallocZone_")
                || line.starts_with("TOTAL ")
        })
        .map(compact_whitespace)
        .collect::<Vec<_>>();

    (!lines.is_empty()).then(|| lines.join(" | "))
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn default_probe_dir(pid: u32) -> PathBuf {
    env::temp_dir().join(format!("crosspuck-probe-{pid}"))
}

fn env_bool(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn env_duration_ms(name: &str) -> Option<Duration> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
}

fn env_duration_secs(name: &str) -> Option<Duration> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn env_i32(name: &str) -> Option<i32> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
