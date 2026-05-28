use crosspuck_core::hid::{
    open_path_with_new_api, snapshot_for_filter, HidCollectionInfo, HidCollectionRole, HidDevice,
    HidFilter, HidSnapshotError, PuckSnapshot,
};
use crosspuck_core::protocol::{
    session_trace_label, CollectionRole, FeatureResult, GetFeature, IdentityPayload, SetFeature,
    SetFeatureResult, SetOutput, SetOutputResult, StatusCode, WriteReport, WriteResult,
};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const RUMBLE_STOP: [u8; 10] = [0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
const COMMAND_OFF: [u8; 4] = [0x82, 0x03, 0x00, 0x00];
const FEATURE_REPORT_ATTEMPTS: usize = 5;
const FEATURE_REPORT_RETRY_DELAY: Duration = Duration::from_millis(2);
const INPUT_REOPEN_BACKOFF: Duration = Duration::from_millis(25);
const INPUT_ERROR_GRACE: Duration = Duration::from_secs(30);
const INPUT_DISCONNECT_GRACE: Duration = Duration::from_secs(120);

static HOST_LOG_SESSION_TRACE_ID: Mutex<Option<u32>> = Mutex::new(None);

pub(crate) fn set_host_log_session_trace_id(session_trace_id: Option<u32>) {
    if let Ok(mut current) = HOST_LOG_SESSION_TRACE_ID.lock() {
        *current = session_trace_id;
    }
}

macro_rules! warn_host_guest_event {
    ($($arg:tt)*) => {
        log_host_guest_warning(format_args!($($arg)*))
    };
}

fn log_host_guest_warning(args: std::fmt::Arguments<'_>) {
    match HOST_LOG_SESSION_TRACE_ID
        .lock()
        .ok()
        .and_then(|guard| *guard)
    {
        Some(session_trace_id) => {
            log::warn!(
                "CrossPuck[{}] {args}",
                session_trace_label(session_trace_id)
            );
        }
        None => log::warn!("CrossPuck {args}"),
    }
}

pub(crate) trait HostBackend: Send + Sync {
    fn identity(&self) -> &IdentityPayload;
    fn open_input_reader(&self) -> Result<Box<dyn InputReportReader>, HostHidError>;
    fn get_feature(&self, request: &GetFeature) -> FeatureResult;
    fn set_feature(&self, request: &SetFeature) -> SetFeatureResult;
    fn set_output(&self, request: &SetOutput) -> SetOutputResult;
    fn write_report(&self, request: &WriteReport) -> WriteResult;
    fn cleanup_feedback(&self);
}

pub(crate) trait InputReportReader: Send {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<HostInputReport>, HostHidError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InputDescriptor {
    pub interface_number: u8,
    pub role: CollectionRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostInputReport {
    pub descriptor: InputDescriptor,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct RealHostBackend {
    snapshot: Arc<Mutex<PuckSnapshot>>,
    identity: IdentityPayload,
    main: SharedMainDevice,
    interface_devices: Arc<Mutex<BTreeMap<u8, CachedInterfaceDevice>>>,
}

#[derive(Clone, Debug)]
struct SharedMainDevice {
    interface_number: u8,
    path: Arc<Mutex<String>>,
    command_device: Arc<Mutex<HidDevice>>,
}

#[derive(Debug)]
struct CachedInterfaceDevice {
    path: String,
    device: HidDevice,
}

impl RealHostBackend {
    pub fn new(snapshot: PuckSnapshot, identity: IdentityPayload) -> Result<Self, HostHidError> {
        let main = Self::open_main_device(&snapshot)?;
        Ok(Self {
            snapshot: Arc::new(Mutex::new(snapshot)),
            identity,
            main,
            interface_devices: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    fn path_in_snapshot(snapshot: &PuckSnapshot, interface_number: u8) -> Option<String> {
        snapshot
            .collections
            .iter()
            .find(|collection| collection.interface_number == i32::from(interface_number))
            .map(|collection| collection.path.clone())
    }

    fn cached_path_for_interface(&self, interface_number: u8) -> Option<String> {
        let snapshot = self.snapshot.lock().ok()?;
        Self::path_in_snapshot(&snapshot, interface_number)
    }

    fn refresh_path_for_interface(&self, interface_number: u8) -> Option<String> {
        let snapshot = snapshot_for_filter(&HidFilter::steam_puck()).ok()?;
        let path = Self::path_in_snapshot(&snapshot, interface_number)?;
        if let Ok(mut current) = self.snapshot.lock() {
            *current = snapshot;
        }
        Some(path)
    }

    fn candidate_paths_for_interface(&self, interface_number: u8) -> Vec<String> {
        let mut paths = Vec::new();
        if let Some(path) = self.cached_path_for_interface(interface_number) {
            paths.push(path);
        }
        if let Some(path) = self.refresh_path_for_interface(interface_number) {
            push_unique_path(&mut paths, path);
        }
        paths
    }

    fn main_path(&self) -> String {
        self.main
            .path
            .lock()
            .map(|path| path.clone())
            .unwrap_or_else(|_| "<poisoned>".to_string())
    }

    fn refresh_main_device(&self) -> Result<(), HostHidError> {
        crate::probe::note_hid_main_refresh_attempt();
        let snapshot = snapshot_for_filter(&HidFilter::steam_puck())?;
        let path = Self::path_in_snapshot(&snapshot, self.main.interface_number)
            .ok_or(HostHidError::MissingCollection(HidCollectionRole::PuckMain))?;
        crate::probe::note_hid_open_path_attempt();
        let device = open_path_with_new_api(&path)?;

        *self
            .main
            .command_device
            .lock()
            .map_err(|_| HostHidError::DeviceLockPoisoned)? = device;
        *self
            .main
            .path
            .lock()
            .map_err(|_| HostHidError::DeviceLockPoisoned)? = path;
        if let Ok(mut current) = self.snapshot.lock() {
            *current = snapshot;
        }
        crate::probe::note_hid_main_refresh_ok();
        Ok(())
    }

    fn open_main_device(snapshot: &PuckSnapshot) -> Result<SharedMainDevice, HostHidError> {
        let collection = snapshot
            .collections
            .iter()
            .find(|collection| collection.role == HidCollectionRole::PuckMain)
            .ok_or(HostHidError::MissingCollection(HidCollectionRole::PuckMain))?;
        let interface_number = u8::try_from(collection.interface_number)
            .map_err(|_| HostHidError::InvalidInterfaceNumber(collection.interface_number))?;
        crate::probe::note_hid_open_path_attempt();
        let device = open_path_with_new_api(&collection.path)?;
        Ok(SharedMainDevice {
            interface_number,
            path: Arc::new(Mutex::new(collection.path.clone())),
            command_device: Arc::new(Mutex::new(device)),
        })
    }

    fn is_main_interface(&self, interface_number: u8) -> bool {
        interface_number == self.main.interface_number
    }

    fn open_input_devices(&self) -> Vec<CollectionInputReader> {
        let Ok(snapshot) = self.snapshot.lock().map(|snapshot| snapshot.clone()) else {
            return Vec::new();
        };
        let mut readers = Vec::new();
        for collection in &snapshot.collections {
            if !should_read_input_collection(collection) {
                continue;
            }
            match self.open_input_device_for_collection(collection) {
                Ok(reader) => readers.push(reader),
                Err(error) => warn_host_guest_event!(
                    "HID input open failed: interface={} role={:?} path={} error={}",
                    collection.interface_number,
                    collection.role,
                    collection.path,
                    error
                ),
            }
        }
        readers
    }

    fn open_input_device_for_collection(
        &self,
        collection: &HidCollectionInfo,
    ) -> Result<CollectionInputReader, HostHidError> {
        let interface_number = u8::try_from(collection.interface_number)
            .map_err(|_| HostHidError::InvalidInterfaceNumber(collection.interface_number))?;
        let descriptor = InputDescriptor {
            interface_number,
            role: CollectionRole::from(collection.role),
        };
        if self.is_main_interface(interface_number) {
            crate::probe::note_hid_open_path_attempt();
            let device = open_path_with_new_api(&collection.path)?;
            return Ok(CollectionInputReader::new(
                self.clone(),
                descriptor,
                collection.input_report_len,
                Arc::new(Mutex::new(collection.path.clone())),
                Arc::new(Mutex::new(device)),
            ));
        }

        crate::probe::note_hid_open_path_attempt();
        let device = open_path_with_new_api(&collection.path)?;
        Ok(CollectionInputReader::new(
            self.clone(),
            descriptor,
            collection.input_report_len,
            Arc::new(Mutex::new(collection.path.clone())),
            Arc::new(Mutex::new(device)),
        ))
    }

    fn open_input_device_for_interface(
        &self,
        interface_number: u8,
        role: CollectionRole,
    ) -> Result<(String, u16, HidDevice), HostHidError> {
        crate::probe::note_hid_interface_reopen_attempt();
        let snapshot = snapshot_for_filter(&HidFilter::steam_puck())?;
        let collection = snapshot
            .collections
            .iter()
            .find(|collection| {
                collection.interface_number == i32::from(interface_number)
                    && CollectionRole::from(collection.role) == role
            })
            .ok_or_else(|| HostHidError::MissingCollection(hid_role_for_protocol_role(role)))?;
        let path = collection.path.clone();
        let input_report_len = collection.input_report_len;
        crate::probe::note_hid_open_path_attempt();
        let device = open_path_with_new_api(&path)?;
        if let Ok(mut current) = self.snapshot.lock() {
            *current = snapshot;
        }
        crate::probe::note_hid_interface_reopen_ok();
        Ok((path, input_report_len, device))
    }

    fn write_raw(&self, interface_number: u8, data: &[u8]) -> WriteResult {
        if self.is_main_interface(interface_number) {
            let mut last_error = None;
            for attempt in 0..FEATURE_REPORT_ATTEMPTS {
                if attempt > 0 {
                    thread::sleep(FEATURE_REPORT_RETRY_DELAY);
                }
                match self.main.command_device.lock() {
                    Ok(device) => match device.write(data) {
                        Ok(written) => {
                            return WriteResult {
                                status: StatusCode::Ok,
                                bytes_written: saturating_u16(written),
                                os_error: 0,
                            };
                        }
                        Err(error) => {
                            last_error = Some(error.to_string());
                        }
                    },
                    Err(_) => last_error = Some("HID device lock poisoned".to_string()),
                }
                let _ = self.refresh_main_device();
            }

            warn_host_guest_event!(
                "HID write failed: interface={} path={} len={} error={}",
                interface_number,
                self.main_path(),
                data.len(),
                last_error.unwrap_or_else(|| "unknown HID write error".to_string())
            );
            return WriteResult {
                status: StatusCode::HidIoError,
                bytes_written: 0,
                os_error: 0,
            };
        }

        match self.write_interface_with_retry(interface_number, data) {
            Ok(written) => WriteResult {
                status: StatusCode::Ok,
                bytes_written: saturating_u16(written),
                os_error: 0,
            },
            Err(error) if error.paths.is_empty() => WriteResult {
                status: StatusCode::UnsupportedInterface,
                bytes_written: 0,
                os_error: 0,
            },
            Err(error) => {
                warn_host_guest_event!(
                    "HID write failed: interface={} paths={} len={} error={}",
                    interface_number,
                    describe_paths(&error.paths),
                    data.len(),
                    error.message
                );
                WriteResult {
                    status: StatusCode::HidIoError,
                    bytes_written: 0,
                    os_error: 0,
                }
            }
        }
    }

    fn open_cached_interface_device(
        &self,
        interface_number: u8,
    ) -> Result<CachedInterfaceDevice, FeatureReportError> {
        let paths = self.candidate_paths_for_interface(interface_number);
        if paths.is_empty() {
            return Err(FeatureReportError {
                paths,
                message: "no matching HID path".to_string(),
            });
        }

        let mut last_error = None;
        for path in &paths {
            crate::probe::note_hid_open_path_attempt();
            match open_path_with_new_api(path) {
                Ok(device) => {
                    return Ok(CachedInterfaceDevice {
                        path: path.clone(),
                        device,
                    });
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }

        Err(FeatureReportError {
            paths,
            message: last_error.unwrap_or_else(|| "no matching HID path".to_string()),
        })
    }

    fn cached_interface_device<'a>(
        &'a self,
        devices: &'a mut BTreeMap<u8, CachedInterfaceDevice>,
        interface_number: u8,
    ) -> Result<&'a mut CachedInterfaceDevice, FeatureReportError> {
        if !devices.contains_key(&interface_number) {
            let device = self.open_cached_interface_device(interface_number)?;
            devices.insert(interface_number, device);
        }
        Ok(devices
            .get_mut(&interface_number)
            .expect("cached interface device must exist after insertion"))
    }

    fn write_interface_with_retry(
        &self,
        interface_number: u8,
        data: &[u8],
    ) -> Result<usize, FeatureReportError> {
        let mut last_error = None;
        let mut last_paths = Vec::new();
        for attempt in 0..FEATURE_REPORT_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(FEATURE_REPORT_RETRY_DELAY);
            }
            let mut devices = self
                .interface_devices
                .lock()
                .map_err(|_| FeatureReportError {
                    paths: last_paths.clone(),
                    message: "HID interface device cache lock poisoned".to_string(),
                })?;
            let device = self.cached_interface_device(&mut devices, interface_number)?;
            last_paths = vec![device.path.clone()];
            match device.device.write(data) {
                Ok(written) => return Ok(written),
                Err(error) => {
                    last_error = Some(error.to_string());
                    devices.remove(&interface_number);
                }
            }
        }

        Err(FeatureReportError {
            paths: last_paths,
            message: last_error.unwrap_or_else(|| "unknown HID write error".to_string()),
        })
    }
}

impl HostBackend for RealHostBackend {
    fn identity(&self) -> &IdentityPayload {
        &self.identity
    }

    fn open_input_reader(&self) -> Result<Box<dyn InputReportReader>, HostHidError> {
        let readers = self.open_input_devices();
        if readers.is_empty() {
            return Err(HostHidError::MissingCollection(HidCollectionRole::PuckMain));
        }
        Ok(Box::new(SharedInputReportReader {
            readers,
            next_index: 0,
        }))
    }

    fn get_feature(&self, request: &GetFeature) -> FeatureResult {
        if request.requested_len == 0 {
            return FeatureResult {
                status: StatusCode::BadRequest,
                os_error: 0,
                data: Vec::new(),
            };
        }
        let mut buffer = vec![0_u8; request.requested_len as usize];
        let result = if self.is_main_interface(request.interface_number) {
            self.get_main_feature_report_with_retry(request.report_id, &mut buffer)
        } else {
            self.get_interface_feature_report_with_retry(
                request.interface_number,
                request.report_id,
                &mut buffer,
            )
        };
        match result {
            Ok(read) => FeatureResult {
                status: StatusCode::Ok,
                os_error: 0,
                data: buffer[..read.min(buffer.len())].to_vec(),
            },
            Err(error) => {
                warn_host_guest_event!(
                    "HID get_feature failed: interface={} paths={} report_id=0x{:02X} len={} error={}",
                    request.interface_number,
                    describe_paths(&error.paths),
                    request.report_id,
                    request.requested_len,
                    error.message
                );
                FeatureResult {
                    status: StatusCode::HidIoError,
                    os_error: 0,
                    data: Vec::new(),
                }
            }
        }
    }

    fn set_feature(&self, request: &SetFeature) -> SetFeatureResult {
        let result = if self.is_main_interface(request.interface_number) {
            self.send_main_feature_report_with_retry(request.interface_number, &request.data)
        } else {
            self.send_interface_feature_report_with_retry(request.interface_number, &request.data)
        };
        match result {
            Ok(accepted) => SetFeatureResult {
                status: StatusCode::Ok,
                bytes_accepted: saturating_u16(accepted),
                os_error: 0,
            },
            Err(error) => {
                if should_accept_transient_set_feature_error(
                    request.interface_number,
                    &request.data,
                    &error.message,
                ) {
                    return SetFeatureResult {
                        status: StatusCode::Ok,
                        bytes_accepted: saturating_u16(request.data.len()),
                        os_error: 0,
                    };
                }
                warn_host_guest_event!(
                    "HID set_feature failed: interface={} paths={} len={} error={}",
                    request.interface_number,
                    describe_paths(&error.paths),
                    request.data.len(),
                    error.message
                );
                SetFeatureResult {
                    status: StatusCode::HidIoError,
                    bytes_accepted: 0,
                    os_error: 0,
                }
            }
        }
    }

    fn set_output(&self, request: &SetOutput) -> SetOutputResult {
        let write = self.write_raw(request.interface_number, &request.data);
        SetOutputResult {
            status: write.status,
            bytes_accepted: write.bytes_written,
            os_error: write.os_error,
        }
    }

    fn write_report(&self, request: &WriteReport) -> WriteResult {
        self.write_raw(request.interface_number, &request.data)
    }

    fn cleanup_feedback(&self) {
        if let Ok(device) = self.main.command_device.lock() {
            let _ = device.write(&RUMBLE_STOP);
            let _ = device.write(&COMMAND_OFF);
        }
    }
}

impl RealHostBackend {
    fn get_main_feature_report_with_retry(
        &self,
        report_id: u8,
        buffer: &mut [u8],
    ) -> Result<usize, FeatureReportError> {
        let mut last_error = None;
        for attempt in 0..FEATURE_REPORT_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(FEATURE_REPORT_RETRY_DELAY);
            }
            buffer.fill(0);
            if let Some(first) = buffer.first_mut() {
                *first = report_id;
            }

            match self.main.command_device.lock() {
                Ok(device) => match device.get_feature_report(buffer) {
                    Ok(read) => return Ok(read),
                    Err(error) => last_error = Some(error.to_string()),
                },
                Err(_) => last_error = Some("HID device lock poisoned".to_string()),
            }
            let _ = self.refresh_main_device();
        }

        Err(FeatureReportError {
            paths: vec![self.main_path()],
            message: last_error.unwrap_or_else(|| "unknown HID get_feature error".to_string()),
        })
    }

    fn send_main_feature_report_with_retry(
        &self,
        interface_number: u8,
        data: &[u8],
    ) -> Result<usize, FeatureReportError> {
        let mut last_error = None;
        for attempt in 0..FEATURE_REPORT_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(FEATURE_REPORT_RETRY_DELAY);
            }

            match self.main.command_device.lock() {
                Ok(device) => match device.send_feature_report(data) {
                    Ok(()) => return Ok(data.len()),
                    Err(error) => {
                        let message = error.to_string();
                        if should_accept_transient_set_feature_error(
                            interface_number,
                            data,
                            &message,
                        ) {
                            return Ok(data.len());
                        }
                        last_error = Some(message);
                    }
                },
                Err(_) => last_error = Some("HID device lock poisoned".to_string()),
            }
            let _ = self.refresh_main_device();
        }

        Err(FeatureReportError {
            paths: vec![self.main_path()],
            message: last_error.unwrap_or_else(|| "unknown HID set_feature error".to_string()),
        })
    }

    fn get_interface_feature_report_with_retry(
        &self,
        interface_number: u8,
        report_id: u8,
        buffer: &mut [u8],
    ) -> Result<usize, FeatureReportError> {
        let mut last_error = None;
        let mut last_paths = Vec::new();
        for attempt in 0..FEATURE_REPORT_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(FEATURE_REPORT_RETRY_DELAY);
            }
            let mut devices = self
                .interface_devices
                .lock()
                .map_err(|_| FeatureReportError {
                    paths: last_paths.clone(),
                    message: "HID interface device cache lock poisoned".to_string(),
                })?;
            let device = self.cached_interface_device(&mut devices, interface_number)?;
            last_paths = vec![device.path.clone()];
            buffer.fill(0);
            if let Some(first) = buffer.first_mut() {
                *first = report_id;
            }
            match device.device.get_feature_report(buffer) {
                Ok(read) => return Ok(read),
                Err(error) => {
                    last_error = Some(error.to_string());
                    devices.remove(&interface_number);
                }
            }
        }

        Err(FeatureReportError {
            paths: last_paths,
            message: last_error.unwrap_or_else(|| "no matching HID path".to_string()),
        })
    }

    fn send_interface_feature_report_with_retry(
        &self,
        interface_number: u8,
        data: &[u8],
    ) -> Result<usize, FeatureReportError> {
        let mut last_error = None;
        let mut last_paths = Vec::new();
        for attempt in 0..FEATURE_REPORT_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(FEATURE_REPORT_RETRY_DELAY);
            }
            let mut devices = self
                .interface_devices
                .lock()
                .map_err(|_| FeatureReportError {
                    paths: last_paths.clone(),
                    message: "HID interface device cache lock poisoned".to_string(),
                })?;
            let device = self.cached_interface_device(&mut devices, interface_number)?;
            last_paths = vec![device.path.clone()];
            match device.device.send_feature_report(data) {
                Ok(()) => return Ok(data.len()),
                Err(error) => {
                    let message = error.to_string();
                    if should_accept_transient_set_feature_error(interface_number, data, &message) {
                        return Ok(data.len());
                    }
                    last_error = Some(message);
                    devices.remove(&interface_number);
                }
            }
        }

        Err(FeatureReportError {
            paths: last_paths,
            message: last_error.unwrap_or_else(|| "no matching HID path".to_string()),
        })
    }
}

#[derive(Debug)]
struct FeatureReportError {
    paths: Vec<String>,
    message: String,
}

fn push_unique_path(paths: &mut Vec<String>, path: String) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn describe_paths(paths: &[String]) -> String {
    if paths.is_empty() {
        "-".to_string()
    } else {
        paths.join(",")
    }
}

struct SharedInputReportReader {
    readers: Vec<CollectionInputReader>,
    next_index: usize,
}

impl InputReportReader for SharedInputReportReader {
    fn read_report(&mut self, timeout: Duration) -> Result<Option<HostInputReport>, HostHidError> {
        if self.readers.is_empty() {
            return Err(HostHidError::MissingCollection(HidCollectionRole::PuckMain));
        }

        let deadline = Instant::now() + timeout;
        loop {
            let reader_count = self.readers.len();
            let mut index = self.next_index % reader_count;
            for _ in 0..reader_count {
                self.next_index = (index + 1) % self.readers.len();
                match self.readers[index].read_once(Duration::ZERO) {
                    Ok(Some(report)) => return Ok(Some(report)),
                    Ok(None) => {}
                    Err(error)
                        if self.readers[index].descriptor.role == CollectionRole::PuckMain =>
                    {
                        return Err(error);
                    }
                    Err(_) => {
                        self.readers.remove(index);
                        if self.readers.is_empty() {
                            return Err(HostHidError::MissingCollection(
                                HidCollectionRole::PuckMain,
                            ));
                        }
                        self.next_index = self.next_index.min(self.readers.len() - 1);
                        break;
                    }
                }
                index = self.next_index % self.readers.len();
            }

            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

struct CollectionInputReader {
    backend: RealHostBackend,
    descriptor: InputDescriptor,
    input_report_len: u16,
    path: Arc<Mutex<String>>,
    device: Arc<Mutex<HidDevice>>,
    buffer: Vec<u8>,
    first_error_at: Option<Instant>,
    consecutive_errors: u32,
}

impl CollectionInputReader {
    fn new(
        backend: RealHostBackend,
        descriptor: InputDescriptor,
        input_report_len: u16,
        path: Arc<Mutex<String>>,
        device: Arc<Mutex<HidDevice>>,
    ) -> Self {
        Self {
            backend,
            descriptor,
            input_report_len,
            path,
            device,
            buffer: vec![0_u8; usize::from(input_report_len).max(64)],
            first_error_at: None,
            consecutive_errors: 0,
        }
    }

    fn read_once(&mut self, timeout: Duration) -> Result<Option<HostInputReport>, HostHidError> {
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let read_result = {
            let device = self
                .device
                .lock()
                .map_err(|_| HostHidError::DeviceLockPoisoned)?;
            device
                .read_timeout(&mut self.buffer, timeout_ms)
                .map_err(HidSnapshotError::from)
        };

        match read_result {
            Ok(0) => {
                self.first_error_at = None;
                self.consecutive_errors = 0;
                Ok(None)
            }
            Ok(read) => {
                self.first_error_at = None;
                self.consecutive_errors = 0;
                Ok(Some(HostInputReport {
                    descriptor: self.descriptor,
                    data: self.buffer[..read].to_vec(),
                }))
            }
            Err(error) => {
                let error_message = error.to_string();
                if is_hid_read_timeout_waiting_for_data(&error_message) {
                    self.first_error_at = None;
                    self.consecutive_errors = 0;
                    return Ok(None);
                }
                let grace = if is_hid_read_timeout_device_disconnected(&error_message) {
                    INPUT_DISCONNECT_GRACE
                } else {
                    INPUT_ERROR_GRACE
                };

                let now = Instant::now();
                let first_error_at = *self.first_error_at.get_or_insert(now);
                self.consecutive_errors = self.consecutive_errors.saturating_add(1);

                if self.consecutive_errors == 1 || self.consecutive_errors.is_multiple_of(40) {
                    warn_host_guest_event!(
                        "HID input read failed: interface={} role={:?} path={} errors={} error={}",
                        self.descriptor.interface_number,
                        self.descriptor.role,
                        self.path_text(),
                        self.consecutive_errors,
                        error
                    );
                }

                if self.consecutive_errors == 1 || self.consecutive_errors.is_multiple_of(4) {
                    crate::probe::note_hid_error_reopen_attempt();
                    match self.refresh_device() {
                        Ok(()) => {
                            crate::probe::note_hid_error_reopen_ok();
                            self.first_error_at = None;
                            self.consecutive_errors = 0;
                        }
                        Err(refresh_error)
                            if self.consecutive_errors == 1
                                || self.consecutive_errors.is_multiple_of(40) =>
                        {
                            warn_host_guest_event!(
                                "HID input reopen failed: interface={} role={:?} path={} error={}",
                                self.descriptor.interface_number,
                                self.descriptor.role,
                                self.path_text(),
                                refresh_error
                            );
                        }
                        Err(_) => {}
                    }
                }

                if now.duration_since(first_error_at) >= grace {
                    return Err(error.into());
                }

                thread::sleep(INPUT_REOPEN_BACKOFF);
                Ok(None)
            }
        }
    }

    fn refresh_device(&mut self) -> Result<(), HostHidError> {
        if self
            .backend
            .is_main_interface(self.descriptor.interface_number)
        {
            let (path, input_report_len, device) = self.backend.open_input_device_for_interface(
                self.descriptor.interface_number,
                self.descriptor.role,
            )?;
            *self
                .device
                .lock()
                .map_err(|_| HostHidError::DeviceLockPoisoned)? = device;
            *self
                .path
                .lock()
                .map_err(|_| HostHidError::DeviceLockPoisoned)? = path;
            self.resize_buffer(input_report_len);
            return Ok(());
        }

        let (path, input_report_len, device) = self.backend.open_input_device_for_interface(
            self.descriptor.interface_number,
            self.descriptor.role,
        )?;
        *self
            .device
            .lock()
            .map_err(|_| HostHidError::DeviceLockPoisoned)? = device;
        *self
            .path
            .lock()
            .map_err(|_| HostHidError::DeviceLockPoisoned)? = path;
        self.resize_buffer(input_report_len);
        Ok(())
    }

    fn resize_buffer(&mut self, input_report_len: u16) {
        self.input_report_len = input_report_len;
        self.buffer.resize(usize::from(input_report_len).max(64), 0);
    }

    fn path_text(&self) -> String {
        self.path
            .lock()
            .map(|path| path.clone())
            .unwrap_or_else(|_| "<poisoned>".to_string())
    }
}

fn should_read_input_collection(collection: &HidCollectionInfo) -> bool {
    collection.input_report_len > 0
        && matches!(
            collection.role,
            HidCollectionRole::PuckMain
                | HidCollectionRole::PuckInterface3
                | HidCollectionRole::PuckInterface4
                | HidCollectionRole::PuckInterface5
        )
}

fn hid_role_for_protocol_role(role: CollectionRole) -> HidCollectionRole {
    match role {
        CollectionRole::PuckMain => HidCollectionRole::PuckMain,
        CollectionRole::PuckInterface3 => HidCollectionRole::PuckInterface3,
        CollectionRole::PuckInterface4 => HidCollectionRole::PuckInterface4,
        CollectionRole::PuckInterface5 => HidCollectionRole::PuckInterface5,
        CollectionRole::PuckVendorDongle => HidCollectionRole::PuckVendorDongle,
    }
}

fn should_accept_transient_set_feature_error(
    interface_number: u8,
    data: &[u8],
    message: &str,
) -> bool {
    if !is_transient_set_feature_error(message) {
        return false;
    }

    let report_id = data.first().copied();
    let command = data.get(1).copied();

    let collection_probe = matches!(interface_number, 3..=5);
    let main_radio_command = interface_number == 2
        && matches!(report_id, Some(0x01 | 0x02))
        && command.is_some_and(|id| id >= 0x80);

    collection_probe || main_radio_command
}

fn is_transient_set_feature_error(message: &str) -> bool {
    message.contains("IOHIDDeviceSetReport failed")
        || message.contains("device not responding")
        || message.contains("Device is disconnected")
        || message.contains("unknown error code")
}

fn is_hid_read_timeout_waiting_for_data(message: &str) -> bool {
    message.contains("hid_read_timeout:") && message.contains("error waiting for more data")
}

fn is_hid_read_timeout_device_disconnected(message: &str) -> bool {
    message.contains("hid_read_timeout:") && message.contains("device disconnected")
}

fn saturating_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

#[derive(Debug)]
pub(crate) enum HostHidError {
    MissingCollection(HidCollectionRole),
    InvalidInterfaceNumber(i32),
    DeviceLockPoisoned,
    Hid(HidSnapshotError),
}

impl fmt::Display for HostHidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCollection(role) => write!(f, "missing HID collection: {}", role.label()),
            Self::InvalidInterfaceNumber(interface_number) => {
                write!(f, "invalid HID interface number: {interface_number}")
            }
            Self::DeviceLockPoisoned => write!(f, "HID device lock poisoned"),
            Self::Hid(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HostHidError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hid(error) => Some(error),
            _ => None,
        }
    }
}

impl From<HidSnapshotError> for HostHidError {
    fn from(value: HidSnapshotError) -> Self {
        Self::Hid(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_transient_collection_probe_errors() {
        assert!(should_accept_transient_set_feature_error(
            3,
            &[0x02, 0xA3, 0x00],
            "hidapi error: IOHIDDeviceSetReport failed: (0xE0005000) unknown error code"
        ));
    }

    #[test]
    fn accepts_known_transient_collection_control_errors() {
        assert!(should_accept_transient_set_feature_error(
            3,
            &[0x02, 0xAE, 0x15, 0x01],
            "hidapi error: IOHIDDeviceSetReport failed: (0xE0005000) unknown error code"
        ));
    }

    #[test]
    fn accepts_known_transient_main_radio_command_errors() {
        assert!(should_accept_transient_set_feature_error(
            2,
            &[0x01, 0xAD, 0x02, 0x01, 0x3C],
            "hidapi error: IOHIDDeviceSetReport failed: (0xE00002ED) device not responding"
        ));
    }

    #[test]
    fn accepts_known_transient_disconnected_main_radio_command_errors() {
        assert!(should_accept_transient_set_feature_error(
            2,
            &[0x01, 0xAE, 0x15, 0x04],
            "hidapi error: Device is disconnected"
        ));
    }

    #[test]
    fn accepts_known_transient_main_report_two_command_errors() {
        assert!(should_accept_transient_set_feature_error(
            2,
            &[0x02, 0xB4, 0x00, 0x00],
            "hidapi error: IOHIDDeviceSetReport failed: (0xE00002ED) device not responding"
        ));
    }

    #[test]
    fn does_not_accept_open_path_errors() {
        assert!(!should_accept_transient_set_feature_error(
            2,
            &[0x01, 0xAD, 0x02, 0x01, 0x3C],
            "hid_open_path: device mach entry not found with the given path"
        ));
    }

    #[test]
    fn treats_macos_waiting_for_more_data_as_timeout() {
        assert!(is_hid_read_timeout_waiting_for_data(
            "hidapi error: hid_read_timeout:  error waiting for more data"
        ));
    }

    #[test]
    fn detects_macos_read_device_disconnect() {
        assert!(is_hid_read_timeout_device_disconnected(
            "hidapi error: hid_read_timeout: device disconnected"
        ));
    }
}
