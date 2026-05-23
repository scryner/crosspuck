pub mod replay;

#[cfg(windows)]
mod windows_proxy {
    use crate::replay::{ReplayPlayer, ReplayScript};
    use min_hook_rs::{create_hook_api, enable_hook, initialize};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::ptr;
    use std::slice;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use windows_sys::core::{BOOL, GUID, PCSTR, PCWSTR};
    use windows_sys::Win32::Foundation::{
        SetLastError, DEVPROPKEY, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, FALSE, HANDLE,
        HINSTANCE, HWND, LPARAM, TRUE,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;
    use windows_sys::Win32::System::LibraryLoader::{
        DisableThreadLibraryCalls, GetModuleFileNameW, GetProcAddress, LoadLibraryW,
    };
    use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::System::IO::OVERLAPPED;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    const TRUE_U8: u8 = 1;
    const FALSE_U8: u8 = 0;
    const VIRTUAL_MAIN_HANDLE_VALUE: isize = -0x4350_5543;
    const VIRTUAL_IF3_HANDLE_VALUE: isize = -0x4350_5545;
    const VIRTUAL_IF4_HANDLE_VALUE: isize = -0x4350_5546;
    const VIRTUAL_IF5_HANDLE_VALUE: isize = -0x4350_5547;
    const VIRTUAL_VENDOR_HANDLE_VALUE: isize = -0x4350_5544;
    const FAKE_MAIN_PREPARSED_VALUE: usize = 0x4350_5543_4850_4D41;
    const FAKE_IF3_PREPARSED_VALUE: usize = 0x4350_5543_4850_4933;
    const FAKE_IF4_PREPARSED_VALUE: usize = 0x4350_5543_4850_4934;
    const FAKE_IF5_PREPARSED_VALUE: usize = 0x4350_5543_4850_4935;
    const FAKE_VENDOR_PREPARSED_VALUE: usize = 0x4350_5543_4850_564E;
    const VIRTUAL_INTERFACE_RESERVED_BASE: usize = 0x4350_5543_5349_0000;
    const SPINT_ACTIVE: u32 = 0x0000_0001;
    const COMPILED_REPLAY_ENABLED: bool = true;
    const COMPILED_REPLAY_DELAY_MS: u64 = 60_000;
    const DEFAULT_IDLE_READ_DELAY_MS: u64 = 16;
    const DEFAULT_IDLE_READ_MAX_REPORTS: usize = 0;
    const DEFAULT_TRACE_REPORT_LIMIT: usize = 256;
    const DEFAULT_TRACE_REPORT_MAX_BYTES: usize = 128;
    const DEFAULT_WINDOW_WATCHDOG_START_DELAY_MS: u64 = 10_000;
    const DEFAULT_WINDOW_WATCHDOG_NO_VISIBLE_GRACE_MS: u64 = 2_500;
    const DEFAULT_WINDOW_WATCHDOG_INTERVAL_MS: u64 = 500;
    const SDL_INIT_VIDEO: u32 = 0x0000_0020;
    const SDL_INIT_JOYSTICK: u32 = 0x0000_0200;
    const SDL_INIT_HAPTIC: u32 = 0x0000_1000;
    const SDL_INIT_GAMEPAD: u32 = 0x0000_2000;
    const SDL_INIT_SENSOR: u32 = 0x0000_8000;
    const SDL_INPUT_SUBSYSTEM_MASK: u32 =
        SDL_INIT_JOYSTICK | SDL_INIT_HAPTIC | SDL_INIT_GAMEPAD | SDL_INIT_SENSOR;
    const ERROR_DEVICE_NOT_CONNECTED_CODE: u32 = 1167;
    const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;
    const REG_SZ: u32 = 1;
    const REG_MULTI_SZ: u32 = 7;
    const DEVPROP_TYPE_STRING: u32 = 18;
    const DEVPROP_TYPE_STRING_LIST: u32 = 8210;
    const SPDRP_DEVICEDESC: u32 = 0x0000_0000;
    const SPDRP_HARDWAREID: u32 = 0x0000_0001;
    const SPDRP_COMPATIBLEIDS: u32 = 0x0000_0002;
    const SPDRP_SERVICE: u32 = 0x0000_0004;
    const SPDRP_CLASS: u32 = 0x0000_0007;
    const SPDRP_MFG: u32 = 0x0000_000B;
    const SPDRP_FRIENDLYNAME: u32 = 0x0000_000C;
    const SPDRP_ENUMERATOR_NAME: u32 = 0x0000_0016;
    const SPDRP_LOCATION_PATHS: u32 = 0x0000_0023;
    const EMBEDDED_REPLAY_JSONL: &str = include_str!("../../../captures/a_button_taps.jsonl");
    const EMBEDDED_RECOGNITION_JSONL: &str =
        include_str!("../../../captures/power_on_idle_70s.jsonl");
    const VIRTUAL_VENDOR_ID: u16 = 0x28DE;
    const VIRTUAL_PRODUCT_ID: u16 = 0x1304;
    const VIRTUAL_VERSION_NUMBER: u16 = 0x0002;
    const VIRTUAL_MANUFACTURER: &str = "Valve Software";
    const VIRTUAL_PRODUCT: &str = "Steam Controller Puck";
    const VIRTUAL_SERIAL: &str = "FXB9961303C9C";
    const HID_INTERFACE_GUID_STRING: &str = "{4d1e55b2-f16f-11cf-88cb-001111000030}";
    const DEVPKEY_DEVICE_DEVICE_DESC: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 2,
    };
    const DEVPKEY_DEVICE_HARDWARE_IDS: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 3,
    };
    const DEVPKEY_DEVICE_COMPATIBLE_IDS: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 4,
    };
    const DEVPKEY_DEVICE_SERVICE: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 6,
    };
    const DEVPKEY_DEVICE_CLASS: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 9,
    };
    const DEVPKEY_DEVICE_MANUFACTURER: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 13,
    };
    const DEVPKEY_DEVICE_FRIENDLY_NAME: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };
    const DEVPKEY_DEVICE_ENUMERATOR_NAME: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 24,
    };
    const DEVPKEY_DEVICE_LOCATION_PATHS: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 37,
    };
    const DEVPKEY_DEVICE_BUS_REPORTED_DEVICE_DESC: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x540b947e_8b40_45bc_a8a2_6a0b894cbda2),
        pid: 4,
    };
    const DEVPKEY_DEVICE_INSTANCE_ID: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x78c34fc8_104a_4aca_9ea4_524d52996e57),
        pid: 256,
    };

    type ReadFileFn =
        unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut OVERLAPPED) -> BOOL;
    type WriteFileFn =
        unsafe extern "system" fn(HANDLE, *const c_void, u32, *mut u32, *mut OVERLAPPED) -> BOOL;
    type DeviceIoControlFn = unsafe extern "system" fn(
        HANDLE,
        u32,
        *mut c_void,
        u32,
        *mut c_void,
        u32,
        *mut u32,
        *mut OVERLAPPED,
    ) -> BOOL;
    type GetProcAddressFn =
        unsafe extern "system" fn(HINSTANCE, PCSTR) -> Option<unsafe extern "system" fn()>;
    type ExitProcessFn = unsafe extern "system" fn(u32);
    type PostQuitMessageFn = unsafe extern "system" fn(i32);
    type SdlQuitFn = unsafe extern "C" fn();
    type SdlQuitSubSystemFn = unsafe extern "C" fn(u32);
    type SdlOpaqueCloseFn = unsafe extern "C" fn(*mut c_void);
    type SdlHidEnumerateFn = unsafe extern "C" fn(u16, u16) -> *mut SdlHidDeviceInfo;
    type SdlHidFreeEnumerationFn = unsafe extern "C" fn(*mut SdlHidDeviceInfo);
    type SdlHidOpenPathFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    type SdlHidReadTimeoutFn = unsafe extern "C" fn(*mut c_void, *mut u8, usize, c_int) -> c_int;
    type SdlHidWriteFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;
    type SdlHidGetFeatureReportFn = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> c_int;
    type SdlHidSendFeatureReportFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;
    type CreateFileWFn = unsafe extern "system" fn(
        PCWSTR,
        u32,
        u32,
        *const SECURITY_ATTRIBUTES,
        u32,
        u32,
        HANDLE,
    ) -> HANDLE;
    type CreateFileAFn = unsafe extern "system" fn(
        PCSTR,
        u32,
        u32,
        *const SECURITY_ATTRIBUTES,
        u32,
        u32,
        HANDLE,
    ) -> HANDLE;
    type CloseHandleFn = unsafe extern "system" fn(HANDLE) -> BOOL;
    type SetupDiGetClassDevsWFn =
        unsafe extern "system" fn(*const GUID, PCWSTR, HANDLE, u32) -> HANDLE;
    type SetupDiGetClassDevsAFn =
        unsafe extern "system" fn(*const GUID, PCSTR, HANDLE, u32) -> HANDLE;
    type SetupDiEnumDeviceInterfacesFn =
        unsafe extern "system" fn(HANDLE, *mut c_void, *const GUID, u32, *mut c_void) -> BOOL;
    type SetupDiGetDeviceInterfaceDetailWFn = unsafe extern "system" fn(
        HANDLE,
        *mut c_void,
        *mut c_void,
        u32,
        *mut u32,
        *mut c_void,
    ) -> BOOL;
    type SetupDiGetDeviceInterfaceDetailAFn = unsafe extern "system" fn(
        HANDLE,
        *mut c_void,
        *mut c_void,
        u32,
        *mut u32,
        *mut c_void,
    ) -> BOOL;
    type SetupDiEnumDeviceInfoFn = unsafe extern "system" fn(HANDLE, u32, *mut c_void) -> BOOL;
    type SetupDiGetDeviceRegistryPropertyWFn = unsafe extern "system" fn(
        HANDLE,
        *mut c_void,
        u32,
        *mut u32,
        *mut u8,
        u32,
        *mut u32,
    ) -> BOOL;
    type SetupDiGetDeviceRegistryPropertyAFn = unsafe extern "system" fn(
        HANDLE,
        *mut c_void,
        u32,
        *mut u32,
        *mut u8,
        u32,
        *mut u32,
    ) -> BOOL;
    type SetupDiGetDeviceInstanceIdAFn =
        unsafe extern "system" fn(HANDLE, *mut c_void, *mut u8, u32, *mut u32) -> BOOL;
    type SetupDiGetDeviceInstanceIdWFn =
        unsafe extern "system" fn(HANDLE, *mut c_void, *mut u16, u32, *mut u32) -> BOOL;
    type SetupDiGetDevicePropertyWFn = unsafe extern "system" fn(
        HANDLE,
        *mut c_void,
        *const DEVPROPKEY,
        *mut u32,
        *mut u8,
        u32,
        *mut u32,
        u32,
    ) -> BOOL;

    static CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();
    static PLAYER: OnceLock<Mutex<ReplayPlayer>> = OnceLock::new();
    static RECOGNITION_PLAYER: OnceLock<Mutex<ReplayPlayer>> = OnceLock::new();
    static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
    static REAL_HID_MODULE: OnceLock<usize> = OnceLock::new();
    static ORIGINAL_READ_FILE: OnceLock<ReadFileFn> = OnceLock::new();
    static ORIGINAL_WRITE_FILE: OnceLock<WriteFileFn> = OnceLock::new();
    static ORIGINAL_DEVICE_IO_CONTROL: OnceLock<DeviceIoControlFn> = OnceLock::new();
    static ORIGINAL_GET_PROC_ADDRESS: OnceLock<GetProcAddressFn> = OnceLock::new();
    static ORIGINAL_EXIT_PROCESS: OnceLock<ExitProcessFn> = OnceLock::new();
    static ORIGINAL_POST_QUIT_MESSAGE: OnceLock<PostQuitMessageFn> = OnceLock::new();
    static ORIGINAL_SDL_QUIT: OnceLock<SdlQuitFn> = OnceLock::new();
    static ORIGINAL_SDL_QUIT_SUB_SYSTEM: OnceLock<SdlQuitSubSystemFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_CLOSE: OnceLock<SdlOpaqueCloseFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_ENUMERATE: OnceLock<SdlHidEnumerateFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_FREE_ENUMERATION: OnceLock<SdlHidFreeEnumerationFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_OPEN_PATH: OnceLock<SdlHidOpenPathFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_READ_TIMEOUT: OnceLock<SdlHidReadTimeoutFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_WRITE: OnceLock<SdlHidWriteFn> = OnceLock::new();
    static ORIGINAL_SDL_HID_GET_FEATURE_REPORT: OnceLock<SdlHidGetFeatureReportFn> =
        OnceLock::new();
    static ORIGINAL_SDL_HID_SEND_FEATURE_REPORT: OnceLock<SdlHidSendFeatureReportFn> =
        OnceLock::new();
    static ORIGINAL_SDL_CLOSE_GAMEPAD: OnceLock<SdlOpaqueCloseFn> = OnceLock::new();
    static ORIGINAL_SDL_CLOSE_JOYSTICK: OnceLock<SdlOpaqueCloseFn> = OnceLock::new();
    static ORIGINAL_CREATE_FILE_W: OnceLock<CreateFileWFn> = OnceLock::new();
    static ORIGINAL_CREATE_FILE_A: OnceLock<CreateFileAFn> = OnceLock::new();
    static ORIGINAL_CLOSE_HANDLE: OnceLock<CloseHandleFn> = OnceLock::new();
    static ORIGINAL_SETUPDI_GET_CLASS_DEVS_W: OnceLock<SetupDiGetClassDevsWFn> = OnceLock::new();
    static ORIGINAL_SETUPDI_GET_CLASS_DEVS_A: OnceLock<SetupDiGetClassDevsAFn> = OnceLock::new();
    static ORIGINAL_SETUPDI_ENUM_DEVICE_INTERFACES: OnceLock<SetupDiEnumDeviceInterfacesFn> =
        OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_W: OnceLock<
        SetupDiGetDeviceInterfaceDetailWFn,
    > = OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_A: OnceLock<
        SetupDiGetDeviceInterfaceDetailAFn,
    > = OnceLock::new();
    static ORIGINAL_SETUPDI_ENUM_DEVICE_INFO: OnceLock<SetupDiEnumDeviceInfoFn> = OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_W: OnceLock<
        SetupDiGetDeviceRegistryPropertyWFn,
    > = OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_A: OnceLock<
        SetupDiGetDeviceRegistryPropertyAFn,
    > = OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_A: OnceLock<SetupDiGetDeviceInstanceIdAFn> =
        OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_W: OnceLock<SetupDiGetDeviceInstanceIdWFn> =
        OnceLock::new();
    static ORIGINAL_SETUPDI_GET_DEVICE_PROPERTY_W: OnceLock<SetupDiGetDevicePropertyWFn> =
        OnceLock::new();
    static VIRTUAL_DEVINSTS: OnceLock<Mutex<Vec<(u32, VirtualHidProfile)>>> = OnceLock::new();
    static SYNTHETIC_ENUM_BASES: OnceLock<Mutex<Vec<(usize, u32)>>> = OnceLock::new();
    static CONTROLLER_LOG_HANDLES: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    static SDL_VIRTUAL_HID_DEVICES: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    static SDL_AUGMENTED_ENUMERATIONS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    static SDL_FEATURE_COMMANDS: OnceLock<Mutex<Vec<SdlFeatureCommand>>> = OnceLock::new();
    static PENDING_INPUT_REPORTS: OnceLock<Mutex<VecDeque<PendingInputReport>>> = OnceLock::new();
    static CREATE_FILE_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
    static GET_PROC_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
    static VIRTUAL_IO_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
    static HIDD_REPORT_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
    static SETUPDI_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
    static REPLAY_READ_COUNT: AtomicUsize = AtomicUsize::new(0);
    static REPORT_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
    static MAIN_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static IF3_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static IF4_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static IF5_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static VENDOR_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static VIRTUAL_STREAM_CONNECTED: AtomicBool = AtomicBool::new(true);
    static WINDOW_WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
    static REPLAY_TAKEOVER_ACTIVE: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static LOGGING: Cell<bool> = const { Cell::new(false) };
    }

    #[derive(Clone, Debug)]
    struct SdlFeatureCommand {
        device: usize,
        report_id: u8,
        command: u8,
        request_value: u8,
    }

    #[derive(Clone, Debug)]
    struct PendingInputReport {
        device: usize,
        report: Vec<u8>,
    }

    #[derive(Clone, Debug)]
    struct RuntimeConfig {
        replay_enabled: bool,
        replay_delay: Duration,
        idle_read_delay: Duration,
        idle_read_max_reports: usize,
        claim_all_hid: bool,
        claim_path_substr: Option<String>,
        masquerade_wine_hid: bool,
        masquerade_wine_pids: Vec<String>,
        trace_reports: bool,
        trace_report_limit: usize,
        trace_report_max_bytes: usize,
        disconnect_on_sdl_quit: bool,
        disconnect_on_sdl_video_quit: bool,
        disconnect_on_sdl_hid_close: bool,
        disconnect_on_sdl_controller_close: bool,
        disconnect_on_post_quit_message: bool,
        disconnect_on_exit_process: bool,
        disconnect_on_controller_workitem_exit: bool,
        disconnect_on_steam_assert_dump: bool,
        window_watchdog_enabled: bool,
        window_watchdog_start_delay: Duration,
        window_watchdog_no_visible_grace: Duration,
        window_watchdog_interval: Duration,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum VirtualHidProfile {
        Main,
        Interface3,
        Interface4,
        Interface5,
        VendorDongle,
    }

    impl VirtualHidProfile {
        fn label(self) -> &'static str {
            match self {
                Self::Main => "puck-if2-main",
                Self::Interface3 => "puck-if3",
                Self::Interface4 => "puck-if4",
                Self::Interface5 => "puck-if5",
                Self::VendorDongle => "puck-vendor-dongle",
            }
        }

        fn handle(self) -> HANDLE {
            match self {
                Self::Main => VIRTUAL_MAIN_HANDLE_VALUE as HANDLE,
                Self::Interface3 => VIRTUAL_IF3_HANDLE_VALUE as HANDLE,
                Self::Interface4 => VIRTUAL_IF4_HANDLE_VALUE as HANDLE,
                Self::Interface5 => VIRTUAL_IF5_HANDLE_VALUE as HANDLE,
                Self::VendorDongle => VIRTUAL_VENDOR_HANDLE_VALUE as HANDLE,
            }
        }

        fn sdl_device(self) -> *mut c_void {
            self.handle() as *mut c_void
        }

        fn preparsed_data(self) -> *mut c_void {
            match self {
                Self::Main => FAKE_MAIN_PREPARSED_VALUE as *mut c_void,
                Self::Interface3 => FAKE_IF3_PREPARSED_VALUE as *mut c_void,
                Self::Interface4 => FAKE_IF4_PREPARSED_VALUE as *mut c_void,
                Self::Interface5 => FAKE_IF5_PREPARSED_VALUE as *mut c_void,
                Self::VendorDongle => FAKE_VENDOR_PREPARSED_VALUE as *mut c_void,
            }
        }

        fn caps(self) -> HidpCaps {
            match self {
                // Current native Steam sees Puck interfaces 2-5 as Generic Desktop
                // top-level collections before wireless controller recognition.
                Self::Main | Self::Interface3 | Self::Interface4 | Self::Interface5 => HidpCaps {
                    usage: 0x0001,
                    usage_page: 0x0001,
                    input_report_byte_length: 54,
                    output_report_byte_length: 64,
                    feature_report_byte_length: 64,
                    reserved: [0; 17],
                    number_link_collection_nodes: 4,
                    number_input_button_caps: 0,
                    number_input_value_caps: 1,
                    number_input_data_indices: 1,
                    number_output_button_caps: 0,
                    number_output_value_caps: 1,
                    number_output_data_indices: 1,
                    number_feature_button_caps: 0,
                    number_feature_value_caps: 1,
                    number_feature_data_indices: 1,
                },
                // Host adapter/vendor HID interface:
                // UsagePage=0xFF00, Usage=0x0002, Input=54, Output=1, Feature=64.
                Self::VendorDongle => HidpCaps {
                    usage: 0x0002,
                    usage_page: 0xFF00,
                    input_report_byte_length: 54,
                    output_report_byte_length: 1,
                    feature_report_byte_length: 64,
                    reserved: [0; 17],
                    number_link_collection_nodes: 1,
                    number_input_button_caps: 0,
                    number_input_value_caps: 1,
                    number_input_data_indices: 1,
                    number_output_button_caps: 0,
                    number_output_value_caps: 0,
                    number_output_data_indices: 0,
                    number_feature_button_caps: 0,
                    number_feature_value_caps: 1,
                    number_feature_data_indices: 1,
                },
            }
        }

        fn interface_number(self) -> u8 {
            match self {
                Self::Main => 2,
                Self::Interface3 => 3,
                Self::Interface4 => 4,
                Self::Interface5 => 5,
                Self::VendorDongle => 6,
            }
        }

        fn collection_number(self) -> u8 {
            match self {
                Self::VendorDongle => 2,
                _ => 1,
            }
        }

        fn instance_suffix(self) -> u8 {
            match self {
                Self::Main => 1,
                Self::Interface3 => 2,
                Self::Interface4 => 3,
                Self::Interface5 => 4,
                Self::VendorDongle => 5,
            }
        }

        fn synthetic_devinst(self) -> u32 {
            0x4350_0000 | u32::from(self.interface_number())
        }

        fn is_active_controller_slot(self) -> bool {
            matches!(self, Self::Main)
        }

        fn interface_reserved(self) -> usize {
            VIRTUAL_INTERFACE_RESERVED_BASE | self.interface_number() as usize
        }
    }

    impl RuntimeConfig {
        fn from_env() -> Self {
            let replay_enabled = COMPILED_REPLAY_ENABLED;
            let replay_delay = Duration::from_millis(COMPILED_REPLAY_DELAY_MS);
            let idle_read_delay = Duration::from_millis(env_u64(
                "CROSSPUCK_IDLE_READ_DELAY_MS",
                DEFAULT_IDLE_READ_DELAY_MS,
            ));
            let idle_read_max_reports = env_u64(
                "CROSSPUCK_IDLE_READ_MAX_REPORTS",
                DEFAULT_IDLE_READ_MAX_REPORTS as u64,
            ) as usize;
            let claim_all_hid = env_bool("CROSSPUCK_CLAIM_ALL_HID", false);
            let claim_path_substr = std::env::var("CROSSPUCK_CLAIM_PATH_SUBSTR")
                .ok()
                .map(|value| value.to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            let masquerade_wine_hid = env_bool("CROSSPUCK_MASQUERADE_WINE_HID", true);
            let masquerade_wine_pids = std::env::var("CROSSPUCK_MASQUERADE_WINE_PIDS")
                .unwrap_or_else(|_| "0002,0001".to_string())
                .split(',')
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect();
            let trace_reports = env_bool("CROSSPUCK_TRACE_REPORTS", false);
            let trace_report_limit = env_u64(
                "CROSSPUCK_TRACE_REPORT_LIMIT",
                DEFAULT_TRACE_REPORT_LIMIT as u64,
            ) as usize;
            let trace_report_max_bytes = env_u64(
                "CROSSPUCK_TRACE_REPORT_MAX_BYTES",
                DEFAULT_TRACE_REPORT_MAX_BYTES as u64,
            ) as usize;
            let disconnect_on_sdl_quit = env_bool("CROSSPUCK_DISCONNECT_ON_SDL_QUIT", true);
            let disconnect_on_sdl_video_quit =
                env_bool("CROSSPUCK_DISCONNECT_ON_SDL_VIDEO_QUIT", false);
            let disconnect_on_sdl_hid_close =
                env_bool("CROSSPUCK_DISCONNECT_ON_SDL_HID_CLOSE", true);
            let disconnect_on_sdl_controller_close =
                env_bool("CROSSPUCK_DISCONNECT_ON_SDL_CONTROLLER_CLOSE", false);
            let disconnect_on_post_quit_message =
                env_bool("CROSSPUCK_DISCONNECT_ON_POST_QUIT_MESSAGE", true);
            let disconnect_on_exit_process = env_bool("CROSSPUCK_DISCONNECT_ON_EXIT_PROCESS", true);
            let disconnect_on_controller_workitem_exit =
                env_bool("CROSSPUCK_DISCONNECT_ON_CONTROLLER_WORKITEM_EXIT", true);
            let disconnect_on_steam_assert_dump =
                env_bool("CROSSPUCK_DISCONNECT_ON_STEAM_ASSERT_DUMP", false);
            let window_watchdog_enabled = env_bool("CROSSPUCK_WINDOW_WATCHDOG", false);
            let window_watchdog_start_delay = Duration::from_millis(env_u64(
                "CROSSPUCK_WINDOW_WATCHDOG_START_DELAY_MS",
                DEFAULT_WINDOW_WATCHDOG_START_DELAY_MS,
            ));
            let window_watchdog_no_visible_grace = Duration::from_millis(env_u64(
                "CROSSPUCK_WINDOW_WATCHDOG_NO_VISIBLE_GRACE_MS",
                DEFAULT_WINDOW_WATCHDOG_NO_VISIBLE_GRACE_MS,
            ));
            let window_watchdog_interval = Duration::from_millis(env_u64(
                "CROSSPUCK_WINDOW_WATCHDOG_INTERVAL_MS",
                DEFAULT_WINDOW_WATCHDOG_INTERVAL_MS,
            ));

            Self {
                replay_enabled,
                replay_delay,
                idle_read_delay,
                idle_read_max_reports,
                claim_all_hid,
                claim_path_substr,
                masquerade_wine_hid,
                masquerade_wine_pids,
                trace_reports,
                trace_report_limit,
                trace_report_max_bytes,
                disconnect_on_sdl_quit,
                disconnect_on_sdl_video_quit,
                disconnect_on_sdl_hid_close,
                disconnect_on_sdl_controller_close,
                disconnect_on_post_quit_message,
                disconnect_on_exit_process,
                disconnect_on_controller_workitem_exit,
                disconnect_on_steam_assert_dump,
                window_watchdog_enabled,
                window_watchdog_start_delay,
                window_watchdog_no_visible_grace,
                window_watchdog_interval,
            }
        }
    }

    #[repr(C)]
    pub struct HiddAttributes {
        size: u32,
        vendor_id: u16,
        product_id: u16,
        version_number: u16,
    }

    #[repr(C)]
    pub struct HidpCaps {
        usage: u16,
        usage_page: u16,
        input_report_byte_length: u16,
        output_report_byte_length: u16,
        feature_report_byte_length: u16,
        reserved: [u16; 17],
        number_link_collection_nodes: u16,
        number_input_button_caps: u16,
        number_input_value_caps: u16,
        number_input_data_indices: u16,
        number_output_button_caps: u16,
        number_output_value_caps: u16,
        number_output_data_indices: u16,
        number_feature_button_caps: u16,
        number_feature_value_caps: u16,
        number_feature_data_indices: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SpDevinfoData {
        cb_size: u32,
        class_guid: GUID,
        dev_inst: u32,
        reserved: usize,
    }

    #[repr(C)]
    struct SpDeviceInterfaceData {
        cb_size: u32,
        interface_class_guid: GUID,
        flags: u32,
        reserved: usize,
    }

    #[repr(C)]
    struct SdlHidDeviceInfo {
        path: *mut c_char,
        vendor_id: u16,
        product_id: u16,
        serial_number: *mut u16,
        release_number: u16,
        manufacturer_string: *mut u16,
        product_string: *mut u16,
        usage_page: u16,
        usage: u16,
        interface_number: c_int,
        interface_class: c_int,
        interface_subclass: c_int,
        interface_protocol: c_int,
        bus_type: c_int,
        next: *mut SdlHidDeviceInfo,
    }

    struct RegistryValue {
        reg_type: u32,
        entries: Vec<&'static str>,
    }

    struct DevicePropertyValue {
        prop_type: u32,
        entries: Vec<&'static str>,
    }

    #[no_mangle]
    pub unsafe extern "system" fn DllMain(
        hinst: HINSTANCE,
        reason: u32,
        _reserved: *mut c_void,
    ) -> BOOL {
        if reason == DLL_PROCESS_ATTACH {
            DisableThreadLibraryCalls(hinst);
            let hinst_value = hinst as usize;
            std::thread::spawn(move || {
                if let Err(error) = initialize_proxy(hinst_value) {
                    debug_line(&format!("[crosspuck] init failed: {error}"));
                }
            });
        } else if reason == DLL_PROCESS_DETACH {
            disconnect_virtual_stream_quiet();
        }
        TRUE
    }

    fn initialize_proxy(hinst_value: usize) -> Result<(), String> {
        initialize_log_path(hinst_value);

        let config = RuntimeConfig::from_env();
        debug_line(&format!(
            "[crosspuck] loading embedded replay enabled={} delay_ms={} idle_read_delay_ms={} idle_read_max_reports={}",
            config.replay_enabled,
            config.replay_delay.as_millis(),
            config.idle_read_delay.as_millis(),
            config.idle_read_max_reports
        ));
        debug_line(&format!(
            "[crosspuck] config claim_all_hid={} claim_path_substr={:?} masquerade_wine_hid={} masquerade_wine_pids={:?}",
            config.claim_all_hid,
            config.claim_path_substr,
            config.masquerade_wine_hid,
            config.masquerade_wine_pids
        ));
        debug_line(&format!(
            "[crosspuck] trace_reports={} trace_report_limit={} trace_report_max_bytes={}",
            config.trace_reports, config.trace_report_limit, config.trace_report_max_bytes
        ));
        debug_line(&format!(
            "[crosspuck] shutdown disconnect_on_sdl_quit={} disconnect_on_sdl_video_quit={} disconnect_on_sdl_hid_close={} disconnect_on_sdl_controller_close={} disconnect_on_post_quit_message={} disconnect_on_exit_process={} disconnect_on_controller_workitem_exit={} disconnect_on_steam_assert_dump={}",
            config.disconnect_on_sdl_quit,
            config.disconnect_on_sdl_video_quit,
            config.disconnect_on_sdl_hid_close,
            config.disconnect_on_sdl_controller_close,
            config.disconnect_on_post_quit_message,
            config.disconnect_on_exit_process,
            config.disconnect_on_controller_workitem_exit,
            config.disconnect_on_steam_assert_dump
        ));
        debug_line(&format!(
            "[crosspuck] window_watchdog enabled={} start_delay_ms={} no_visible_grace_ms={} interval_ms={}",
            config.window_watchdog_enabled,
            config.window_watchdog_start_delay.as_millis(),
            config.window_watchdog_no_visible_grace.as_millis(),
            config.window_watchdog_interval.as_millis()
        ));
        debug_line(&format!(
            "[crosspuck] virtual identity vid=0x{VIRTUAL_VENDOR_ID:04X} pid=0x{VIRTUAL_PRODUCT_ID:04X} version=0x{VIRTUAL_VERSION_NUMBER:04X} manufacturer={VIRTUAL_MANUFACTURER:?} product={VIRTUAL_PRODUCT:?} serial={VIRTUAL_SERIAL:?}"
        ));

        CONFIG
            .set(config.clone())
            .map_err(|_| "runtime config already initialized".to_string())?;
        VIRTUAL_STREAM_CONNECTED.store(true, Ordering::Relaxed);
        let replay_script =
            ReplayScript::from_jsonl(EMBEDDED_REPLAY_JSONL).map_err(|error| error.to_string())?;
        let packet_count = replay_script.len();
        let recognition_script = ReplayScript::from_jsonl(EMBEDDED_RECOGNITION_JSONL)
            .map_err(|error| error.to_string())?;
        let recognition_packet_count = recognition_script.len();
        RECOGNITION_PLAYER
            .set(Mutex::new(ReplayPlayer::new(
                recognition_script,
                Duration::ZERO,
            )))
            .map_err(|_| "recognition player already initialized".to_string())?;
        PLAYER
            .set(Mutex::new(ReplayPlayer::new(
                replay_script,
                config.replay_delay,
            )))
            .map_err(|_| "replay player already initialized".to_string())?;
        debug_line(&format!(
            "[crosspuck] loaded embedded recognition stream packets={} source=power_on_idle_70s.jsonl",
            recognition_packet_count
        ));

        initialize().map_err(|error| format!("hook initialize failed: {error:?}"))?;
        install_hook(
            "kernel32.dll",
            "ReadFile",
            detoured_read_file as *mut c_void,
            |ptr| {
                ORIGINAL_READ_FILE
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "ReadFile trampoline already initialized".to_string())
            },
        )?;
        install_hook(
            "kernel32.dll",
            "WriteFile",
            detoured_write_file as *mut c_void,
            |ptr| {
                ORIGINAL_WRITE_FILE
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "WriteFile trampoline already initialized".to_string())
            },
        )?;
        install_hook(
            "kernel32.dll",
            "DeviceIoControl",
            detoured_device_io_control as *mut c_void,
            |ptr| {
                ORIGINAL_DEVICE_IO_CONTROL
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "DeviceIoControl trampoline already initialized".to_string())
            },
        )?;
        install_hook(
            "kernel32.dll",
            "CreateFileW",
            detoured_create_file_w as *mut c_void,
            |ptr| {
                ORIGINAL_CREATE_FILE_W
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "CreateFileW trampoline already initialized".to_string())
            },
        )?;
        install_hook(
            "kernel32.dll",
            "CreateFileA",
            detoured_create_file_a as *mut c_void,
            |ptr| {
                ORIGINAL_CREATE_FILE_A
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "CreateFileA trampoline already initialized".to_string())
            },
        )?;
        install_hook(
            "kernel32.dll",
            "CloseHandle",
            detoured_close_handle as *mut c_void,
            |ptr| {
                ORIGINAL_CLOSE_HANDLE
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "CloseHandle trampoline already initialized".to_string())
            },
        )?;
        install_hook(
            "kernel32.dll",
            "GetProcAddress",
            detoured_get_proc_address as *mut c_void,
            |ptr| {
                ORIGINAL_GET_PROC_ADDRESS
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "GetProcAddress trampoline already initialized".to_string())
            },
        )?;
        install_lifecycle_shutdown_hooks(&config);
        install_setupapi_diagnostic_hooks();
        if config.disconnect_on_sdl_quit {
            install_sdl_shutdown_hooks();
        }
        start_window_watchdog(config.clone());

        debug_line(&format!(
            "[crosspuck] proxy ready; {} packets queued; replay_enabled={} replay starts after {}ms",
            packet_count,
            config.replay_enabled,
            config.replay_delay.as_millis()
        ));
        Ok(())
    }

    fn install_hook<F>(
        module: &str,
        proc_name: &str,
        detour: *mut c_void,
        set_original: F,
    ) -> Result<(), String>
    where
        F: FnOnce(*mut c_void) -> Result<(), String>,
    {
        let (trampoline, target) = create_hook_api(module, proc_name, detour)
            .map_err(|error| format!("create hook {module}!{proc_name} failed: {error:?}"))?;
        set_original(trampoline)?;
        enable_hook(target)
            .map_err(|error| format!("enable hook {module}!{proc_name} failed: {error:?}"))?;
        debug_line(&format!("[crosspuck] hook enabled {module}!{proc_name}"));
        Ok(())
    }

    fn install_optional_hook<F>(module: &str, proc_name: &str, detour: *mut c_void, set_original: F)
    where
        F: FnOnce(*mut c_void) -> Result<(), String>,
    {
        match install_hook(module, proc_name, detour, set_original) {
            Ok(()) => {}
            Err(error) => debug_line(&format!("[crosspuck] optional hook skipped: {error}")),
        }
    }

    fn install_lifecycle_shutdown_hooks(config: &RuntimeConfig) {
        if config.disconnect_on_exit_process {
            install_optional_hook(
                "kernel32.dll",
                "ExitProcess",
                detoured_exit_process as *mut c_void,
                |ptr| {
                    ORIGINAL_EXIT_PROCESS
                        .set(unsafe { std::mem::transmute(ptr) })
                        .map_err(|_| "ExitProcess trampoline already initialized".to_string())
                },
            );
        }

        if config.disconnect_on_post_quit_message {
            load_library("user32.dll");
            install_optional_hook(
                "user32.dll",
                "PostQuitMessage",
                detoured_post_quit_message as *mut c_void,
                |ptr| {
                    ORIGINAL_POST_QUIT_MESSAGE
                        .set(unsafe { std::mem::transmute(ptr) })
                        .map_err(|_| "PostQuitMessage trampoline already initialized".to_string())
                },
            );
        }
    }

    fn install_setupapi_diagnostic_hooks() {
        load_library("setupapi.dll");
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetClassDevsW",
            detoured_setupdi_get_class_devs_w as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_CLASS_DEVS_W
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SetupDiGetClassDevsW trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetClassDevsA",
            detoured_setupdi_get_class_devs_a as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_CLASS_DEVS_A
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SetupDiGetClassDevsA trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiEnumDeviceInterfaces",
            detoured_setupdi_enum_device_interfaces as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_ENUM_DEVICE_INTERFACES
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiEnumDeviceInterfaces trampoline already initialized".to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDeviceInterfaceDetailW",
            detoured_setupdi_get_device_interface_detail_w as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_W
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDeviceInterfaceDetailW trampoline already initialized"
                            .to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDeviceInterfaceDetailA",
            detoured_setupdi_get_device_interface_detail_a as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_A
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDeviceInterfaceDetailA trampoline already initialized"
                            .to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiEnumDeviceInfo",
            detoured_setupdi_enum_device_info as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_ENUM_DEVICE_INFO
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SetupDiEnumDeviceInfo trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDeviceRegistryPropertyW",
            detoured_setupdi_get_device_registry_property_w as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_W
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDeviceRegistryPropertyW trampoline already initialized"
                            .to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDeviceRegistryPropertyA",
            detoured_setupdi_get_device_registry_property_a as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_A
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDeviceRegistryPropertyA trampoline already initialized"
                            .to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDeviceInstanceIdA",
            detoured_setupdi_get_device_instance_id_a as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_A
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDeviceInstanceIdA trampoline already initialized".to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDeviceInstanceIdW",
            detoured_setupdi_get_device_instance_id_w as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_W
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDeviceInstanceIdW trampoline already initialized".to_string()
                    })
            },
        );
        install_optional_hook(
            "setupapi.dll",
            "SetupDiGetDevicePropertyW",
            detoured_setupdi_get_device_property_w as *mut c_void,
            |ptr| {
                ORIGINAL_SETUPDI_GET_DEVICE_PROPERTY_W
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SetupDiGetDevicePropertyW trampoline already initialized".to_string()
                    })
            },
        );
    }

    fn install_sdl_shutdown_hooks() {
        let module = load_library("SDL3.dll");
        debug_line(&format!(
            "[crosspuck] SDL3.dll load for shutdown hooks -> 0x{module:016X}"
        ));
        install_optional_hook(
            "SDL3.dll",
            "SDL_Quit",
            detoured_sdl_quit as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_QUIT
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_Quit trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_QuitSubSystem",
            detoured_sdl_quit_sub_system as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_QUIT_SUB_SYSTEM
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_QuitSubSystem trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_close",
            detoured_sdl_hid_close as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_CLOSE
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_hid_close trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_enumerate",
            detoured_sdl_hid_enumerate as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_ENUMERATE
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_hid_enumerate trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_free_enumeration",
            detoured_sdl_hid_free_enumeration as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_FREE_ENUMERATION
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SDL_hid_free_enumeration trampoline already initialized".to_string()
                    })
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_open_path",
            detoured_sdl_hid_open_path as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_OPEN_PATH
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_hid_open_path trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_read_timeout",
            detoured_sdl_hid_read_timeout as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_READ_TIMEOUT
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_hid_read_timeout trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_write",
            detoured_sdl_hid_write as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_WRITE
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_hid_write trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_get_feature_report",
            detoured_sdl_hid_get_feature_report as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_GET_FEATURE_REPORT
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SDL_hid_get_feature_report trampoline already initialized".to_string()
                    })
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_hid_send_feature_report",
            detoured_sdl_hid_send_feature_report as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_HID_SEND_FEATURE_REPORT
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| {
                        "SDL_hid_send_feature_report trampoline already initialized".to_string()
                    })
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_CloseGamepad",
            detoured_sdl_close_gamepad as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_CLOSE_GAMEPAD
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_CloseGamepad trampoline already initialized".to_string())
            },
        );
        install_optional_hook(
            "SDL3.dll",
            "SDL_CloseJoystick",
            detoured_sdl_close_joystick as *mut c_void,
            |ptr| {
                ORIGINAL_SDL_CLOSE_JOYSTICK
                    .set(unsafe { std::mem::transmute(ptr) })
                    .map_err(|_| "SDL_CloseJoystick trampoline already initialized".to_string())
            },
        );
    }

    fn start_window_watchdog(config: RuntimeConfig) {
        if !config.window_watchdog_enabled {
            return;
        }
        if WINDOW_WATCHDOG_STARTED.swap(true, Ordering::Relaxed) {
            return;
        }

        let pid = unsafe { GetCurrentProcessId() };
        std::thread::spawn(move || {
            std::thread::sleep(config.window_watchdog_start_delay);
            debug_line(&format!(
                "[crosspuck] window watchdog started pid={} start_delay_ms={} no_visible_grace_ms={} interval_ms={}",
                pid,
                config.window_watchdog_start_delay.as_millis(),
                config.window_watchdog_no_visible_grace.as_millis(),
                config.window_watchdog_interval.as_millis()
            ));

            let mut no_visible_since: Option<Instant> = None;
            loop {
                if !virtual_stream_connected() {
                    break;
                }

                let active_virtual_session = REPLAY_READ_COUNT.load(Ordering::Relaxed) > 0
                    || virtual_profiles_open_count() > 0;
                if !active_virtual_session {
                    no_visible_since = None;
                    std::thread::sleep(config.window_watchdog_interval);
                    continue;
                }

                if process_has_visible_window(pid) {
                    if no_visible_since.take().is_some() {
                        debug_line("[crosspuck] window watchdog visible window restored");
                    }
                    std::thread::sleep(config.window_watchdog_interval);
                    continue;
                }

                let first_no_visible = no_visible_since.get_or_insert_with(Instant::now);
                let elapsed = first_no_visible.elapsed();
                if elapsed.is_zero() {
                    debug_line(&format!(
                        "[crosspuck] window watchdog no visible window observed open_main={} open_vendor={} reads={}",
                        MAIN_OPEN_COUNT.load(Ordering::Relaxed),
                        VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
                        REPLAY_READ_COUNT.load(Ordering::Relaxed)
                    ));
                }

                if elapsed >= config.window_watchdog_no_visible_grace {
                    debug_line(&format!(
                        "[crosspuck] window watchdog disconnecting after no visible window for {}ms open_main={} open_vendor={} reads={}",
                        elapsed.as_millis(),
                        MAIN_OPEN_COUNT.load(Ordering::Relaxed),
                        VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
                        REPLAY_READ_COUNT.load(Ordering::Relaxed)
                    ));
                    disconnect_virtual_stream("visible window watchdog");
                    break;
                }

                std::thread::sleep(config.window_watchdog_interval);
            }
        });
    }

    #[repr(C)]
    struct WindowScan {
        pid: u32,
        visible: bool,
    }

    fn process_has_visible_window(pid: u32) -> bool {
        let mut scan = WindowScan {
            pid,
            visible: false,
        };
        unsafe {
            EnumWindows(
                Some(enum_windows_for_process),
                &mut scan as *mut WindowScan as LPARAM,
            );
        }
        scan.visible
    }

    unsafe extern "system" fn enum_windows_for_process(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let scan = &mut *(lparam as *mut WindowScan);
        let mut window_pid = 0;
        GetWindowThreadProcessId(hwnd, &mut window_pid);
        if window_pid == scan.pid && IsWindowVisible(hwnd) != FALSE {
            scan.visible = true;
            return FALSE;
        }
        TRUE
    }

    unsafe extern "system" fn detoured_create_file_w(
        file_name: PCWSTR,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const SECURITY_ATTRIBUTES,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: HANDLE,
    ) -> HANDLE {
        if is_logging() {
            return ORIGINAL_CREATE_FILE_W
                .get()
                .copied()
                .map_or(ptr::null_mut(), |original| {
                    original(
                        file_name,
                        desired_access,
                        share_mode,
                        security_attributes,
                        creation_disposition,
                        flags_and_attributes,
                        template_file,
                    )
                });
        }

        let mut controller_log_path = None;
        if let Some(path) = wide_z_to_string(file_name) {
            maybe_disconnect_on_steam_assert_path("CreateFileW", &path);
            if should_claim_path(&path) {
                if !virtual_stream_connected() {
                    debug_line(&format!(
                        "[crosspuck] reject HID path after virtual disconnect: {path}"
                    ));
                    SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
                    return invalid_handle_value();
                }
                let profile = virtual_profile_for_path(&path);
                let handle = profile.handle();
                let open_count = mark_virtual_profile_open(profile);
                debug_line(&format!(
                    "[crosspuck] claimed HID path as {} open_count={}: {path}",
                    handle_label(handle),
                    open_count
                ));
                return handle;
            }
            if looks_like_controller_log_path(&path) {
                controller_log_path = Some(path.clone());
            }
            log_create_file_path("CreateFileW", &path);
        }

        let handle = ORIGINAL_CREATE_FILE_W
            .get()
            .copied()
            .map_or(ptr::null_mut(), |original| {
                original(
                    file_name,
                    desired_access,
                    share_mode,
                    security_attributes,
                    creation_disposition,
                    flags_and_attributes,
                    template_file,
                )
            });
        if let Some(path) = controller_log_path.as_deref() {
            remember_controller_log_handle("CreateFileW", handle, path);
        }
        handle
    }

    unsafe extern "system" fn detoured_create_file_a(
        file_name: PCSTR,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const SECURITY_ATTRIBUTES,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: HANDLE,
    ) -> HANDLE {
        if is_logging() {
            return ORIGINAL_CREATE_FILE_A
                .get()
                .copied()
                .map_or(ptr::null_mut(), |original| {
                    original(
                        file_name,
                        desired_access,
                        share_mode,
                        security_attributes,
                        creation_disposition,
                        flags_and_attributes,
                        template_file,
                    )
                });
        }

        let mut controller_log_path = None;
        if let Some(path) = narrow_z_to_string(file_name) {
            maybe_disconnect_on_steam_assert_path("CreateFileA", &path);
            if should_claim_path(&path) {
                if !virtual_stream_connected() {
                    debug_line(&format!(
                        "[crosspuck] reject HID path after virtual disconnect: {path}"
                    ));
                    SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
                    return invalid_handle_value();
                }
                let profile = virtual_profile_for_path(&path);
                let handle = profile.handle();
                let open_count = mark_virtual_profile_open(profile);
                debug_line(&format!(
                    "[crosspuck] claimed HID path as {} open_count={}: {path}",
                    handle_label(handle),
                    open_count
                ));
                return handle;
            }
            if looks_like_controller_log_path(&path) {
                controller_log_path = Some(path.clone());
            }
            log_create_file_path("CreateFileA", &path);
        }

        let handle = ORIGINAL_CREATE_FILE_A
            .get()
            .copied()
            .map_or(ptr::null_mut(), |original| {
                original(
                    file_name,
                    desired_access,
                    share_mode,
                    security_attributes,
                    creation_disposition,
                    flags_and_attributes,
                    template_file,
                )
            });
        if let Some(path) = controller_log_path.as_deref() {
            remember_controller_log_handle("CreateFileA", handle, path);
        }
        handle
    }

    unsafe extern "system" fn detoured_read_file(
        file: HANDLE,
        buffer: *mut c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
        overlapped: *mut OVERLAPPED,
    ) -> BOOL {
        if is_virtual_handle(file) {
            if !virtual_handle_is_open(file) {
                if !bytes_read.is_null() {
                    *bytes_read = 0;
                }
                SetLastError(ERROR_INVALID_HANDLE);
                return FALSE;
            }
            if !virtual_stream_connected() {
                if !bytes_read.is_null() {
                    *bytes_read = 0;
                }
                SetLastError(ERROR_DEVICE_NOT_CONNECTED_CODE);
                return FALSE;
            }
            let Some(profile) = virtual_profile_for_handle(file) else {
                return FALSE;
            };
            return read_virtual_hid_report(profile, buffer, bytes_to_read, bytes_read);
        }

        ORIGINAL_READ_FILE.get().copied().map_or(FALSE, |original| {
            original(file, buffer, bytes_to_read, bytes_read, overlapped)
        })
    }

    unsafe extern "system" fn detoured_write_file(
        file: HANDLE,
        buffer: *const c_void,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut OVERLAPPED,
    ) -> BOOL {
        if is_virtual_handle(file) {
            log_virtual_io(&format!(
                "[crosspuck] WriteFile virtual handle={} len={} head={}",
                handle_label(file),
                bytes_to_write,
                hex_head(buffer as *const u8, bytes_to_write)
            ));
            trace_virtual_report(
                "WriteFile",
                "request",
                file,
                buffer as *const u8,
                bytes_to_write,
            );
            if !bytes_written.is_null() {
                *bytes_written = bytes_to_write;
            }
            return TRUE;
        }

        maybe_disconnect_on_controller_log_write(file, buffer, bytes_to_write);

        ORIGINAL_WRITE_FILE
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(file, buffer, bytes_to_write, bytes_written, overlapped)
            })
    }

    unsafe extern "system" fn detoured_device_io_control(
        device: HANDLE,
        io_control_code: u32,
        in_buffer: *mut c_void,
        in_buffer_size: u32,
        out_buffer: *mut c_void,
        out_buffer_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut OVERLAPPED,
    ) -> BOOL {
        if is_virtual_handle(device) {
            log_virtual_io(&format!(
                "[crosspuck] DeviceIoControl virtual handle={} code=0x{:08X} in_len={} out_len={} in_head={}",
                handle_label(device),
                io_control_code,
                in_buffer_size,
                out_buffer_size,
                hex_head(in_buffer as *const u8, in_buffer_size)
            ));
            trace_virtual_report(
                "DeviceIoControl",
                "request",
                device,
                in_buffer as *const u8,
                in_buffer_size,
            );
            let report_id = report_id_from_buffer(in_buffer as *const u8, in_buffer_size);
            zero_buffer_with_report_id(out_buffer, out_buffer_size, report_id);
            trace_virtual_report(
                "DeviceIoControl",
                "response",
                device,
                out_buffer as *const u8,
                out_buffer_size,
            );
            if !bytes_returned.is_null() {
                *bytes_returned = out_buffer_size;
            }
            return TRUE;
        }

        ORIGINAL_DEVICE_IO_CONTROL
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device,
                    io_control_code,
                    in_buffer,
                    in_buffer_size,
                    out_buffer,
                    out_buffer_size,
                    bytes_returned,
                    overlapped,
                )
            })
    }

    unsafe extern "system" fn detoured_get_proc_address(
        module: HINSTANCE,
        proc_name: PCSTR,
    ) -> Option<unsafe extern "system" fn()> {
        let result = ORIGINAL_GET_PROC_ADDRESS
            .get()
            .copied()
            .and_then(|original| original(module, proc_name));
        let proc_label = proc_name_label(proc_name);
        let module_label = module_file_path(module)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| format!("{module:p}"));
        let proc_lower = proc_label.to_ascii_lowercase();
        let module_lower = module_label.to_ascii_lowercase();
        let interesting = proc_lower.contains("hid")
            || proc_lower.contains("cm_")
            || module_lower.contains("hid.dll")
            || module_lower.contains("sdl3.dll")
            || module_lower.contains("cfgmgr32");
        if interesting {
            let count = GET_PROC_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= 512 || result.is_none() {
                debug_line(&format!(
                    "[crosspuck] GetProcAddress module={module_label:?} proc={proc_label:?} -> {}",
                    if result.is_some() { "FOUND" } else { "NULL" }
                ));
            }
        }
        result
    }

    unsafe extern "system" fn detoured_exit_process(exit_code: u32) {
        let should_disconnect = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_exit_process)
            && active_virtual_session();
        debug_line(&format!(
            "[crosspuck] ExitProcess exit_code={} should_disconnect={} open_main={} open_vendor={} reads={}",
            exit_code,
            should_disconnect,
            MAIN_OPEN_COUNT.load(Ordering::Relaxed),
            VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
            REPLAY_READ_COUNT.load(Ordering::Relaxed)
        ));
        if should_disconnect {
            disconnect_virtual_stream("ExitProcess");
        }
        if let Some(original) = ORIGINAL_EXIT_PROCESS.get().copied() {
            original(exit_code);
        }
    }

    unsafe extern "system" fn detoured_post_quit_message(exit_code: i32) {
        let should_disconnect = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_post_quit_message)
            && active_virtual_session();
        debug_line(&format!(
            "[crosspuck] PostQuitMessage exit_code={} should_disconnect={} open_main={} open_vendor={} reads={}",
            exit_code,
            should_disconnect,
            MAIN_OPEN_COUNT.load(Ordering::Relaxed),
            VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
            REPLAY_READ_COUNT.load(Ordering::Relaxed)
        ));
        if should_disconnect {
            disconnect_virtual_stream("PostQuitMessage");
        }
        if let Some(original) = ORIGINAL_POST_QUIT_MESSAGE.get().copied() {
            original(exit_code);
        }
    }

    unsafe extern "C" fn detoured_sdl_quit() {
        disconnect_virtual_stream("SDL_Quit");
        if let Some(original) = ORIGINAL_SDL_QUIT.get().copied() {
            original();
        }
    }

    unsafe extern "C" fn detoured_sdl_quit_sub_system(flags: u32) {
        let input_shutdown = flags & SDL_INPUT_SUBSYSTEM_MASK != 0;
        let video_shutdown = flags & SDL_INIT_VIDEO != 0;
        debug_line(&format!(
            "[crosspuck] SDL_QuitSubSystem flags=0x{flags:08X} input_shutdown={input_shutdown} video_shutdown={video_shutdown} open_main={} open_vendor={} reads={}",
            MAIN_OPEN_COUNT.load(Ordering::Relaxed),
            VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
            REPLAY_READ_COUNT.load(Ordering::Relaxed)
        ));
        let disconnect_on_video = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_sdl_video_quit);
        if input_shutdown || (video_shutdown && disconnect_on_video) {
            let reason = if input_shutdown {
                "SDL_QuitSubSystem input subsystem"
            } else {
                "SDL_QuitSubSystem video subsystem"
            };
            disconnect_virtual_stream(reason);
        }
        if let Some(original) = ORIGINAL_SDL_QUIT_SUB_SYSTEM.get().copied() {
            original(flags);
        }
    }

    unsafe extern "C" fn detoured_sdl_hid_close(device: *mut c_void) {
        let fake_profile = virtual_profile_for_fake_sdl_device(device);
        forget_sdl_virtual_hid_device(device);
        let should_disconnect = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_sdl_hid_close)
            && active_virtual_session();
        debug_line(&format!(
            "[crosspuck] SDL_hid_close device={device:p} should_disconnect={} open_main={} open_vendor={} reads={}",
            should_disconnect,
            MAIN_OPEN_COUNT.load(Ordering::Relaxed),
            VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
            REPLAY_READ_COUNT.load(Ordering::Relaxed)
        ));
        if should_disconnect {
            disconnect_virtual_stream("SDL_hid_close");
        }
        if fake_profile.is_some() {
            return;
        }
        if let Some(original) = ORIGINAL_SDL_HID_CLOSE.get().copied() {
            original(device);
        }
    }

    unsafe extern "C" fn detoured_sdl_hid_enumerate(
        vendor_id: u16,
        product_id: u16,
    ) -> *mut SdlHidDeviceInfo {
        let original = ORIGINAL_SDL_HID_ENUMERATE
            .get()
            .copied()
            .map_or(ptr::null_mut(), |original| original(vendor_id, product_id));

        if !should_augment_sdl_hid_enumeration(vendor_id, product_id) {
            return original;
        }

        let augmented = augment_sdl_hid_enumeration(original);
        debug_line(&format!(
            "[crosspuck] SDL_hid_enumerate vid=0x{vendor_id:04X} pid=0x{product_id:04X} original={original:p} returned={augmented:p}"
        ));
        augmented
    }

    unsafe extern "C" fn detoured_sdl_hid_free_enumeration(device_info: *mut SdlHidDeviceInfo) {
        if is_augmented_sdl_enumeration(device_info) {
            debug_line(&format!(
                "[crosspuck] SDL_hid_free_enumeration augmented head={device_info:p} leaked for PoC safety"
            ));
            return;
        }

        if let Some(original) = ORIGINAL_SDL_HID_FREE_ENUMERATION.get().copied() {
            original(device_info);
        }
    }

    unsafe extern "C" fn detoured_sdl_hid_open_path(path: *const c_char) -> *mut c_void {
        let path_text = narrow_z_to_string(path as PCSTR);
        if let Some(path) = path_text.as_deref() {
            if should_claim_path(path) && is_valve_puck_path(&path.to_ascii_lowercase()) {
                let profile = virtual_profile_for_path(path);
                let device = profile.sdl_device();
                remember_sdl_virtual_hid_device("SDL_hid_open_path synthetic", device, path);
                debug_line(&format!(
                    "[crosspuck] SDL_hid_open_path synthetic profile={} device={device:p} path={path:?}",
                    profile.label()
                ));
                return device;
            }
        }

        let device = ORIGINAL_SDL_HID_OPEN_PATH
            .get()
            .copied()
            .map_or(ptr::null_mut(), |original| original(path));

        if let Some(path) = path_text.as_deref() {
            if should_claim_path(path) {
                remember_sdl_virtual_hid_device("SDL_hid_open_path", device, path);
            }
        }
        device
    }

    unsafe extern "C" fn detoured_sdl_hid_read_timeout(
        device: *mut c_void,
        data: *mut u8,
        len: usize,
        milliseconds: c_int,
    ) -> c_int {
        if is_sdl_virtual_hid_device(device) {
            let count = if milliseconds == 0 {
                let Some(profile) = virtual_profile_for_fake_sdl_device(device) else {
                    return -1;
                };
                match read_virtual_hid_report_ready(profile, data as *mut c_void, len as u32) {
                    Ok(Some(count)) => count,
                    Ok(None) => return 0,
                    Err(()) => return -1,
                }
            } else {
                let mut bytes_read = 0_u32;
                let Some(profile) = virtual_profile_for_fake_sdl_device(device) else {
                    return -1;
                };
                let result = read_virtual_hid_report(
                    profile,
                    data as *mut c_void,
                    len as u32,
                    &mut bytes_read,
                );
                if result != TRUE {
                    return -1;
                }
                bytes_read as usize
            };
            log_hidd_report_call(&format!(
                "[crosspuck] SDL_hid_read_timeout virtual device={device:p} timeout={} requested={} returned={}",
                milliseconds, len, count
            ));
            return count.min(c_int::MAX as usize) as c_int;
        }

        ORIGINAL_SDL_HID_READ_TIMEOUT
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len, milliseconds))
    }

    unsafe extern "C" fn detoured_sdl_hid_write(
        device: *mut c_void,
        data: *const u8,
        len: usize,
    ) -> c_int {
        if is_sdl_virtual_hid_device(device) {
            debug_line(&format!(
                "[crosspuck] SDL_hid_write virtual device={device:p} len={} head={}",
                len,
                hex_head(data, len as u32)
            ));
            return len.min(c_int::MAX as usize) as c_int;
        }

        ORIGINAL_SDL_HID_WRITE
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len))
    }

    unsafe extern "C" fn detoured_sdl_hid_get_feature_report(
        device: *mut c_void,
        data: *mut u8,
        len: usize,
    ) -> c_int {
        if is_sdl_virtual_hid_device(device) {
            return synthesize_sdl_feature_report("SDL_hid_get_feature_report", device, data, len);
        }

        ORIGINAL_SDL_HID_GET_FEATURE_REPORT
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len))
    }

    unsafe extern "C" fn detoured_sdl_hid_send_feature_report(
        device: *mut c_void,
        data: *const u8,
        len: usize,
    ) -> c_int {
        if is_sdl_virtual_hid_device(device) {
            log_hidd_report_call(&format!(
                "[crosspuck] SDL_hid_send_feature_report virtual device={device:p} len={} head={}",
                len,
                hex_head(data, len as u32)
            ));
            remember_sdl_feature_command(device, data, len);
            return len.min(c_int::MAX as usize) as c_int;
        }

        ORIGINAL_SDL_HID_SEND_FEATURE_REPORT
            .get()
            .copied()
            .map_or(-1, |original| original(device, data, len))
    }

    unsafe extern "C" fn detoured_sdl_close_gamepad(gamepad: *mut c_void) {
        let should_disconnect = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_sdl_controller_close)
            && active_virtual_session();
        debug_line(&format!(
            "[crosspuck] SDL_CloseGamepad gamepad={gamepad:p} should_disconnect={} open_main={} open_vendor={} reads={}",
            should_disconnect,
            MAIN_OPEN_COUNT.load(Ordering::Relaxed),
            VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
            REPLAY_READ_COUNT.load(Ordering::Relaxed)
        ));
        if should_disconnect {
            disconnect_virtual_stream("SDL_CloseGamepad");
        }
        if let Some(original) = ORIGINAL_SDL_CLOSE_GAMEPAD.get().copied() {
            original(gamepad);
        }
    }

    unsafe extern "C" fn detoured_sdl_close_joystick(joystick: *mut c_void) {
        let should_disconnect = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_sdl_controller_close)
            && active_virtual_session();
        debug_line(&format!(
            "[crosspuck] SDL_CloseJoystick joystick={joystick:p} should_disconnect={} open_main={} open_vendor={} reads={}",
            should_disconnect,
            MAIN_OPEN_COUNT.load(Ordering::Relaxed),
            VENDOR_OPEN_COUNT.load(Ordering::Relaxed),
            REPLAY_READ_COUNT.load(Ordering::Relaxed)
        ));
        if should_disconnect {
            disconnect_virtual_stream("SDL_CloseJoystick");
        }
        if let Some(original) = ORIGINAL_SDL_CLOSE_JOYSTICK.get().copied() {
            original(joystick);
        }
    }

    unsafe extern "system" fn detoured_close_handle(handle: HANDLE) -> BOOL {
        if is_virtual_handle(handle) {
            let open_count = mark_virtual_handle_closed(handle);
            debug_line(&format!(
                "[crosspuck] CloseHandle virtual handle={} open_count={}",
                handle_label(handle),
                open_count
            ));
            return TRUE;
        }

        forget_controller_log_handle(handle);

        ORIGINAL_CLOSE_HANDLE
            .get()
            .copied()
            .map_or(FALSE, |original| original(handle))
    }

    unsafe extern "system" fn detoured_setupdi_get_class_devs_w(
        class_guid: *const GUID,
        enumerator: PCWSTR,
        hwnd_parent: HANDLE,
        flags: u32,
    ) -> HANDLE {
        let enumerator_text = wide_z_to_string(enumerator).unwrap_or_default();
        log_setupdi_line(&format!(
            "[crosspuck] SetupDiGetClassDevsW guid={} enumerator={:?} flags=0x{flags:08X}",
            guid_label(class_guid),
            enumerator_text
        ));
        ORIGINAL_SETUPDI_GET_CLASS_DEVS_W
            .get()
            .copied()
            .map_or(ptr::null_mut(), |original| {
                original(class_guid, enumerator, hwnd_parent, flags)
            })
    }

    unsafe extern "system" fn detoured_setupdi_get_class_devs_a(
        class_guid: *const GUID,
        enumerator: PCSTR,
        hwnd_parent: HANDLE,
        flags: u32,
    ) -> HANDLE {
        let enumerator_text = narrow_z_to_string(enumerator).unwrap_or_default();
        log_setupdi_line(&format!(
            "[crosspuck] SetupDiGetClassDevsA guid={} enumerator={:?} flags=0x{flags:08X}",
            guid_label(class_guid),
            enumerator_text
        ));
        ORIGINAL_SETUPDI_GET_CLASS_DEVS_A
            .get()
            .copied()
            .map_or(ptr::null_mut(), |original| {
                original(class_guid, enumerator, hwnd_parent, flags)
            })
    }

    unsafe extern "system" fn detoured_setupdi_enum_device_interfaces(
        device_info_set: HANDLE,
        device_info_data: *mut c_void,
        interface_class_guid: *const GUID,
        member_index: u32,
        device_interface_data: *mut c_void,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_ENUM_DEVICE_INTERFACES
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_info_data,
                    interface_class_guid,
                    member_index,
                    device_interface_data,
                )
            });
        if result == FALSE
            && device_info_data.is_null()
            && is_hid_interface_guid(interface_class_guid)
        {
            if let Some(profile) =
                synthetic_profile_for_failed_member_index(device_info_set, member_index)
            {
                if write_synthetic_device_interface_data(
                    device_interface_data,
                    interface_class_guid,
                    profile,
                ) {
                    debug_line(&format!(
                        "[crosspuck] SetupDiEnumDeviceInterfaces synthetic profile={} index={}",
                        profile.label(),
                        member_index
                    ));
                    return TRUE;
                }
            }
        }
        if result == TRUE || member_index < 4 {
            let message = format!(
                "[crosspuck] SetupDiEnumDeviceInterfaces guid={} index={} -> {}",
                guid_label(interface_class_guid),
                member_index,
                bool_label(result)
            );
            if result == TRUE {
                debug_line(&message);
            } else {
                log_setupdi_line(&message);
            }
        }
        result
    }

    unsafe extern "system" fn detoured_setupdi_get_device_interface_detail_w(
        device_info_set: HANDLE,
        device_interface_data: *mut c_void,
        device_interface_detail_data: *mut c_void,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> BOOL {
        if let Some(profile) = virtual_profile_for_device_interface_data(device_interface_data) {
            return synthesize_device_interface_detail_w(
                profile,
                device_interface_detail_data,
                device_interface_detail_data_size,
                required_size,
                device_info_data,
            );
        }

        let result = ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_W
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_interface_data,
                    device_interface_detail_data,
                    device_interface_detail_data_size,
                    required_size,
                    device_info_data,
                )
            });
        let required = if required_size.is_null() {
            0
        } else {
            *required_size
        };
        let mut path = detail_w_path(device_interface_detail_data).unwrap_or_default();
        if result == TRUE {
            if let Some(rewritten_path) = rewrite_hid_device_path(&path) {
                if write_detail_w_path(
                    device_interface_detail_data,
                    device_interface_detail_data_size,
                    &rewritten_path,
                ) {
                    debug_line(&format!(
                        "[crosspuck] rewritten SetupAPI W path: {path:?} -> {rewritten_path:?}"
                    ));
                    path = rewritten_path;
                } else {
                    debug_line(&format!(
                        "[crosspuck] failed to rewrite SetupAPI W path size={} path={path:?}",
                        device_interface_detail_data_size
                    ));
                }
            }
            if let Some(profile) = virtual_profile_from_text(&path) {
                remember_virtual_device_info(device_info_data, profile, "DetailW");
            }
        }
        if result == TRUE || looks_interesting_path(&path) || device_interface_detail_data_size == 0
        {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInterfaceDetailW size={} required={} -> {} path={:?}",
                device_interface_detail_data_size,
                required,
                bool_label(result),
                path
            ));
        }
        result
    }

    unsafe extern "system" fn detoured_setupdi_get_device_interface_detail_a(
        device_info_set: HANDLE,
        device_interface_data: *mut c_void,
        device_interface_detail_data: *mut c_void,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> BOOL {
        if let Some(profile) = virtual_profile_for_device_interface_data(device_interface_data) {
            return synthesize_device_interface_detail_a(
                profile,
                device_interface_detail_data,
                device_interface_detail_data_size,
                required_size,
                device_info_data,
            );
        }

        let result = ORIGINAL_SETUPDI_GET_DEVICE_INTERFACE_DETAIL_A
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_interface_data,
                    device_interface_detail_data,
                    device_interface_detail_data_size,
                    required_size,
                    device_info_data,
                )
            });
        let required = if required_size.is_null() {
            0
        } else {
            *required_size
        };
        let mut path = detail_a_path(device_interface_detail_data).unwrap_or_default();
        if result == TRUE {
            if let Some(rewritten_path) = rewrite_hid_device_path(&path) {
                if write_detail_a_path(
                    device_interface_detail_data,
                    device_interface_detail_data_size,
                    &rewritten_path,
                ) {
                    debug_line(&format!(
                        "[crosspuck] rewritten SetupAPI A path: {path:?} -> {rewritten_path:?}"
                    ));
                    path = rewritten_path;
                } else {
                    debug_line(&format!(
                        "[crosspuck] failed to rewrite SetupAPI A path size={} path={path:?}",
                        device_interface_detail_data_size
                    ));
                }
            }
            if let Some(profile) = virtual_profile_from_text(&path) {
                remember_virtual_device_info(device_info_data, profile, "DetailA");
            }
        }
        if result == TRUE || looks_interesting_path(&path) || device_interface_detail_data_size == 0
        {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInterfaceDetailA size={} required={} -> {} path={:?}",
                device_interface_detail_data_size,
                required,
                bool_label(result),
                path
            ));
        }
        result
    }

    unsafe extern "system" fn detoured_setupdi_enum_device_info(
        device_info_set: HANDLE,
        member_index: u32,
        device_info_data: *mut c_void,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_ENUM_DEVICE_INFO
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(device_info_set, member_index, device_info_data)
            });
        if result == TRUE || member_index < 8 {
            let devinst = devinst_from_device_info_data(device_info_data)
                .map(|value| format!("0x{value:08X}"))
                .unwrap_or_else(|| "none".to_string());
            debug_line(&format!(
                "[crosspuck] SetupDiEnumDeviceInfo index={} -> {} devinst={}",
                member_index,
                bool_label(result),
                devinst
            ));
        }
        result
    }

    unsafe extern "system" fn detoured_setupdi_get_device_registry_property_w(
        device_info_set: HANDLE,
        device_info_data: *mut c_void,
        property: u32,
        property_reg_data_type: *mut u32,
        property_buffer: *mut u8,
        property_buffer_size: u32,
        required_size: *mut u32,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_W
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_info_data,
                    property,
                    property_reg_data_type,
                    property_buffer,
                    property_buffer_size,
                    required_size,
                )
            });
        let existing = if result == TRUE {
            registry_w_text(property_buffer, property_buffer_size, required_size)
        } else {
            None
        };
        let profile = virtual_profile_for_device_info_data(device_info_data)
            .or_else(|| existing.as_deref().and_then(virtual_profile_from_text));

        let Some(profile) = profile else {
            log_registry_property_result(
                "SetupDiGetDeviceRegistryPropertyW",
                property,
                result,
                existing.as_deref(),
            );
            return result;
        };

        remember_virtual_device_info(device_info_data, profile, "RegistryPropertyW");
        let Some(value) = registry_property_value(profile, property) else {
            log_registry_property_result(
                "SetupDiGetDeviceRegistryPropertyW",
                property,
                result,
                existing.as_deref(),
            );
            return result;
        };

        if !property_reg_data_type.is_null() {
            *property_reg_data_type = value.reg_type;
        }
        let value_text = value.entries.join("|");
        if write_registry_value_w(property_buffer, property_buffer_size, required_size, &value) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceRegistryPropertyW rewrite property={} profile={} type={} value={:?}",
                registry_property_label(property),
                profile.label(),
                registry_type_label(value.reg_type),
                value_text
            ));
            TRUE
        } else {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceRegistryPropertyW needs larger buffer property={} profile={} type={} required={} size={} value={:?}",
                registry_property_label(property),
                profile.label(),
                registry_type_label(value.reg_type),
                required_value_size_w(&value),
                property_buffer_size,
                value_text
            ));
            FALSE
        }
    }

    unsafe extern "system" fn detoured_setupdi_get_device_registry_property_a(
        device_info_set: HANDLE,
        device_info_data: *mut c_void,
        property: u32,
        property_reg_data_type: *mut u32,
        property_buffer: *mut u8,
        property_buffer_size: u32,
        required_size: *mut u32,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_GET_DEVICE_REGISTRY_PROPERTY_A
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_info_data,
                    property,
                    property_reg_data_type,
                    property_buffer,
                    property_buffer_size,
                    required_size,
                )
            });
        let existing = if result == TRUE {
            registry_a_text(property_buffer, property_buffer_size, required_size)
        } else {
            None
        };
        let profile = virtual_profile_for_device_info_data(device_info_data)
            .or_else(|| existing.as_deref().and_then(virtual_profile_from_text));

        let Some(profile) = profile else {
            log_registry_property_result(
                "SetupDiGetDeviceRegistryPropertyA",
                property,
                result,
                existing.as_deref(),
            );
            return result;
        };

        remember_virtual_device_info(device_info_data, profile, "RegistryPropertyA");
        let Some(value) = registry_property_value(profile, property) else {
            log_registry_property_result(
                "SetupDiGetDeviceRegistryPropertyA",
                property,
                result,
                existing.as_deref(),
            );
            return result;
        };

        if !property_reg_data_type.is_null() {
            *property_reg_data_type = value.reg_type;
        }
        let value_text = value.entries.join("|");
        if write_registry_value_a(property_buffer, property_buffer_size, required_size, &value) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceRegistryPropertyA rewrite property={} profile={} type={} value={:?}",
                registry_property_label(property),
                profile.label(),
                registry_type_label(value.reg_type),
                value_text
            ));
            TRUE
        } else {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceRegistryPropertyA needs larger buffer property={} profile={} type={} required={} size={} value={:?}",
                registry_property_label(property),
                profile.label(),
                registry_type_label(value.reg_type),
                required_value_size_a(&value),
                property_buffer_size,
                value_text
            ));
            FALSE
        }
    }

    unsafe extern "system" fn detoured_setupdi_get_device_instance_id_a(
        device_info_set: HANDLE,
        device_info_data: *mut c_void,
        device_instance_id: *mut u8,
        device_instance_id_size: u32,
        required_size: *mut u32,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_A
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_info_data,
                    device_instance_id,
                    device_instance_id_size,
                    required_size,
                )
            });
        let existing = if result == TRUE {
            narrow_z_to_string(device_instance_id)
        } else {
            None
        };
        let profile = virtual_profile_for_device_info_data(device_info_data)
            .or_else(|| existing.as_deref().and_then(virtual_profile_from_text));

        let Some(profile) = profile else {
            if result == TRUE || existing.as_deref().is_some_and(looks_interesting_path) {
                debug_line(&format!(
                    "[crosspuck] SetupDiGetDeviceInstanceIdA -> {} value={:?}",
                    bool_label(result),
                    existing
                ));
            }
            return result;
        };

        remember_virtual_device_info(device_info_data, profile, "InstanceIdA");
        let value = device_instance_id_value(profile);
        if write_instance_id_a(
            device_instance_id,
            device_instance_id_size,
            required_size,
            value,
        ) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInstanceIdA rewrite profile={} value={value:?}",
                profile.label()
            ));
            TRUE
        } else {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInstanceIdA needs larger buffer profile={} required={} size={} value={value:?}",
                profile.label(),
                value.len() + 1,
                device_instance_id_size
            ));
            FALSE
        }
    }

    unsafe extern "system" fn detoured_setupdi_get_device_instance_id_w(
        device_info_set: HANDLE,
        device_info_data: *mut c_void,
        device_instance_id: *mut u16,
        device_instance_id_size: u32,
        required_size: *mut u32,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_GET_DEVICE_INSTANCE_ID_W
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_info_data,
                    device_instance_id,
                    device_instance_id_size,
                    required_size,
                )
            });
        let existing = if result == TRUE {
            wide_z_to_string(device_instance_id)
        } else {
            None
        };
        let profile = virtual_profile_for_device_info_data(device_info_data)
            .or_else(|| existing.as_deref().and_then(virtual_profile_from_text));

        let Some(profile) = profile else {
            if result == TRUE || existing.as_deref().is_some_and(looks_interesting_path) {
                debug_line(&format!(
                    "[crosspuck] SetupDiGetDeviceInstanceIdW -> {} value={:?}",
                    bool_label(result),
                    existing
                ));
            }
            return result;
        };

        remember_virtual_device_info(device_info_data, profile, "InstanceIdW");
        let value = device_instance_id_value(profile);
        if write_instance_id_w(
            device_instance_id,
            device_instance_id_size,
            required_size,
            value,
        ) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInstanceIdW rewrite profile={} value={value:?}",
                profile.label()
            ));
            TRUE
        } else {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInstanceIdW needs larger buffer profile={} required={} size={} value={value:?}",
                profile.label(),
                value.encode_utf16().count() + 1,
                device_instance_id_size
            ));
            FALSE
        }
    }

    unsafe extern "system" fn detoured_setupdi_get_device_property_w(
        device_info_set: HANDLE,
        device_info_data: *mut c_void,
        property_key: *const DEVPROPKEY,
        property_type: *mut u32,
        property_buffer: *mut u8,
        property_buffer_size: u32,
        required_size: *mut u32,
        flags: u32,
    ) -> BOOL {
        let result = ORIGINAL_SETUPDI_GET_DEVICE_PROPERTY_W
            .get()
            .copied()
            .map_or(FALSE, |original| {
                original(
                    device_info_set,
                    device_info_data,
                    property_key,
                    property_type,
                    property_buffer,
                    property_buffer_size,
                    required_size,
                    flags,
                )
            });
        let existing_type = if property_type.is_null() {
            0
        } else {
            *property_type
        };
        let existing = if result == TRUE {
            device_property_w_text(
                property_buffer,
                property_buffer_size,
                required_size,
                existing_type,
            )
        } else {
            None
        };
        let profile = virtual_profile_for_device_info_data(device_info_data)
            .or_else(|| existing.as_deref().and_then(virtual_profile_from_text));
        let key_label = devpropkey_label(property_key);

        let Some(profile) = profile else {
            log_device_property_result(
                "SetupDiGetDevicePropertyW",
                property_key,
                existing_type,
                result,
                existing.as_deref(),
            );
            return result;
        };

        remember_virtual_device_info(device_info_data, profile, "DevicePropertyW");
        let Some(value) = device_property_value(profile, property_key) else {
            log_device_property_result(
                "SetupDiGetDevicePropertyW",
                property_key,
                existing_type,
                result,
                existing.as_deref(),
            );
            return result;
        };

        if !property_type.is_null() {
            *property_type = value.prop_type;
        }
        let value_text = value.entries.join("|");
        if write_device_property_value_w(
            property_buffer,
            property_buffer_size,
            required_size,
            &value,
        ) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDevicePropertyW rewrite key={} profile={} type={} value={:?}",
                key_label,
                profile.label(),
                devprop_type_label(value.prop_type),
                value_text
            ));
            TRUE
        } else {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDevicePropertyW needs larger buffer key={} profile={} type={} required={} size={} value={:?}",
                key_label,
                profile.label(),
                devprop_type_label(value.prop_type),
                required_device_property_size_w(&value),
                property_buffer_size,
                value_text
            ));
            FALSE
        }
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetHidGuid(guid: *mut GUID) {
        debug_line("[crosspuck] HidD_GetHidGuid");
        if guid.is_null() {
            return;
        }

        *guid = hid_interface_guid();
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetAttributes(
        device: HANDLE,
        attributes: *mut HiddAttributes,
    ) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetAttributes handle={}",
            handle_label(device)
        ));
        if virtual_profile_for_handle(device).is_none() {
            return call_real_hidd_get_attributes(device, attributes);
        }

        if attributes.is_null() {
            return FALSE_U8;
        }

        *attributes = HiddAttributes {
            size: std::mem::size_of::<HiddAttributes>() as u32,
            vendor_id: VIRTUAL_VENDOR_ID,
            product_id: VIRTUAL_PRODUCT_ID,
            version_number: VIRTUAL_VERSION_NUMBER,
        };
        debug_line(&format!(
            "[crosspuck] HidD_GetAttributes -> vid=0x{:04X} pid=0x{:04X} version=0x{:04X}",
            VIRTUAL_VENDOR_ID, VIRTUAL_PRODUCT_ID, VIRTUAL_VERSION_NUMBER
        ));
        TRUE_U8
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetPreparsedData(
        device: HANDLE,
        preparsed_data: *mut *mut c_void,
    ) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetPreparsedData handle={}",
            handle_label(device)
        ));
        let Some(profile) = virtual_profile_for_handle(device) else {
            return call_real_hidd_get_preparsed_data(device, preparsed_data);
        };

        if preparsed_data.is_null() {
            return FALSE_U8;
        }

        *preparsed_data = profile.preparsed_data();
        TRUE_U8
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_FreePreparsedData(preparsed_data: *mut c_void) -> u8 {
        let is_fake = virtual_profile_for_preparsed_data(preparsed_data).is_some();
        debug_line(&format!(
            "[crosspuck] HidD_FreePreparsedData fake={}",
            is_fake
        ));
        if is_fake {
            return TRUE_U8;
        }
        call_real_hidd_free_preparsed_data(preparsed_data)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetManufacturerString(
        device: HANDLE,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetManufacturerString handle={} len={}",
            handle_label(device),
            buffer_len
        ));
        if is_virtual_handle(device) {
            debug_line(&format!(
                "[crosspuck] HidD_GetManufacturerString -> {VIRTUAL_MANUFACTURER:?}"
            ));
            return write_wide_string(buffer, buffer_len, VIRTUAL_MANUFACTURER);
        }
        call_real_hidd_string("HidD_GetManufacturerString", device, buffer, buffer_len)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetProductString(
        device: HANDLE,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetProductString handle={} len={}",
            handle_label(device),
            buffer_len
        ));
        if is_virtual_handle(device) {
            debug_line(&format!(
                "[crosspuck] HidD_GetProductString -> {VIRTUAL_PRODUCT:?}"
            ));
            return write_wide_string(buffer, buffer_len, VIRTUAL_PRODUCT);
        }
        call_real_hidd_string("HidD_GetProductString", device, buffer, buffer_len)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetSerialNumberString(
        device: HANDLE,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetSerialNumberString handle={} len={}",
            handle_label(device),
            buffer_len
        ));
        if is_virtual_handle(device) {
            debug_line(&format!(
                "[crosspuck] HidD_GetSerialNumberString -> {VIRTUAL_SERIAL:?}"
            ));
            return write_wide_string(buffer, buffer_len, VIRTUAL_SERIAL);
        }
        call_real_hidd_string("HidD_GetSerialNumberString", device, buffer, buffer_len)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetIndexedString(
        device: HANDLE,
        string_index: u32,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetIndexedString handle={} index={} len={}",
            handle_label(device),
            string_index,
            buffer_len
        ));
        if is_virtual_handle(device) {
            let value = match string_index {
                1 => VIRTUAL_MANUFACTURER,
                2 => VIRTUAL_PRODUCT,
                3 => VIRTUAL_SERIAL,
                _ => VIRTUAL_PRODUCT,
            };
            debug_line(&format!("[crosspuck] HidD_GetIndexedString -> {value:?}"));
            return write_wide_string(buffer, buffer_len, value);
        }
        call_real_hidd_indexed_string(device, string_index, buffer, buffer_len)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetInputReport(
        device: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_len: u32,
    ) -> u8 {
        log_hidd_report_call(&format!(
            "[crosspuck] HidD_GetInputReport handle={} len={}",
            handle_label(device),
            report_buffer_len
        ));
        if is_virtual_handle(device) {
            if !virtual_handle_is_open(device) {
                SetLastError(ERROR_INVALID_HANDLE);
                return FALSE_U8;
            }
            let Some(profile) = virtual_profile_for_handle(device) else {
                return FALSE_U8;
            };
            return bool_to_u8(read_virtual_hid_report(
                profile,
                report_buffer,
                report_buffer_len,
                ptr::null_mut(),
            ));
        }
        call_real_hidd_report(
            "HidD_GetInputReport",
            device,
            report_buffer,
            report_buffer_len,
        )
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetFeature(
        device: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_len: u32,
    ) -> u8 {
        log_hidd_report_call(&format!(
            "[crosspuck] HidD_GetFeature handle={} len={}",
            handle_label(device),
            report_buffer_len
        ));
        if is_virtual_handle(device) {
            trace_virtual_report(
                "HidD_GetFeature",
                "request",
                device,
                report_buffer as *const u8,
                report_buffer_len,
            );
            let report_id = report_id_from_buffer(report_buffer as *const u8, report_buffer_len);
            zero_buffer_with_report_id(report_buffer, report_buffer_len, report_id);
            trace_virtual_report(
                "HidD_GetFeature",
                "response",
                device,
                report_buffer as *const u8,
                report_buffer_len,
            );
            return TRUE_U8;
        }
        call_real_hidd_report("HidD_GetFeature", device, report_buffer, report_buffer_len)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_SetFeature(
        device: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_len: u32,
    ) -> u8 {
        log_hidd_report_call(&format!(
            "[crosspuck] HidD_SetFeature handle={} len={}",
            handle_label(device),
            report_buffer_len
        ));
        if is_virtual_handle(device) {
            trace_virtual_report(
                "HidD_SetFeature",
                "request",
                device,
                report_buffer as *const u8,
                report_buffer_len,
            );
            return TRUE_U8;
        }
        call_real_hidd_report("HidD_SetFeature", device, report_buffer, report_buffer_len)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_SetOutputReport(
        device: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_len: u32,
    ) -> u8 {
        log_hidd_report_call(&format!(
            "[crosspuck] HidD_SetOutputReport handle={} len={}",
            handle_label(device),
            report_buffer_len
        ));
        if is_virtual_handle(device) {
            trace_virtual_report(
                "HidD_SetOutputReport",
                "request",
                device,
                report_buffer as *const u8,
                report_buffer_len,
            );
            return TRUE_U8;
        }
        call_real_hidd_report(
            "HidD_SetOutputReport",
            device,
            report_buffer,
            report_buffer_len,
        )
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_FlushQueue(device: HANDLE) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_FlushQueue handle={}",
            handle_label(device)
        ));
        if is_virtual_handle(device) {
            return TRUE_U8;
        }
        call_real_hidd_handle_bool("HidD_FlushQueue", device)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_SetNumInputBuffers(device: HANDLE, count: u32) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_SetNumInputBuffers handle={} count={}",
            handle_label(device),
            count
        ));
        if is_virtual_handle(device) {
            return TRUE_U8;
        }

        type RealFn = unsafe extern "system" fn(HANDLE, u32) -> u8;
        resolve_real_hid_proc("HidD_SetNumInputBuffers")
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, count))
            .unwrap_or(FALSE_U8)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidD_GetNumInputBuffers(device: HANDLE, count: *mut u32) -> u8 {
        debug_line(&format!(
            "[crosspuck] HidD_GetNumInputBuffers handle={}",
            handle_label(device)
        ));
        if is_virtual_handle(device) {
            if !count.is_null() {
                *count = 64;
            }
            return TRUE_U8;
        }

        type RealFn = unsafe extern "system" fn(HANDLE, *mut u32) -> u8;
        resolve_real_hid_proc("HidD_GetNumInputBuffers")
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, count))
            .unwrap_or(FALSE_U8)
    }

    #[no_mangle]
    pub unsafe extern "system" fn HidP_GetCaps(
        preparsed_data: *mut c_void,
        caps: *mut HidpCaps,
    ) -> i32 {
        debug_line(&format!(
            "[crosspuck] HidP_GetCaps fake={}",
            virtual_profile_for_preparsed_data(preparsed_data).is_some()
        ));
        let Some(profile) = virtual_profile_for_preparsed_data(preparsed_data) else {
            type RealFn = unsafe extern "system" fn(*mut c_void, *mut HidpCaps) -> i32;
            return resolve_real_hid_proc("HidP_GetCaps")
                .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(preparsed_data, caps))
                .unwrap_or(-1);
        };

        if caps.is_null() {
            return -1;
        }

        *caps = profile.caps();
        debug_line(&format!(
            "[crosspuck] HidP_GetCaps -> profile={} usage_page=0x{:04X} usage=0x{:04X} in={} out={} feature={}",
            profile.label(),
            (*caps).usage_page,
            (*caps).usage,
            (*caps).input_report_byte_length,
            (*caps).output_report_byte_length,
            (*caps).feature_report_byte_length
        ));
        HIDP_STATUS_SUCCESS
    }

    fn read_virtual_hid_report(
        profile: VirtualHidProfile,
        buffer: *mut c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
    ) -> BOOL {
        if buffer.is_null() {
            return FALSE;
        }

        if !profile.is_active_controller_slot() {
            if !bytes_read.is_null() {
                unsafe {
                    *bytes_read = 0;
                }
            }
            log_hidd_report_call(&format!(
                "[crosspuck] idle profile read profile={} requested={} returned=0",
                profile.label(),
                bytes_to_read
            ));
            return TRUE;
        }

        if let Some(result) = read_pending_input_report(profile, buffer, bytes_to_read, bytes_read)
        {
            return result;
        }

        if !replay_stream_active() {
            return read_recognition_report(buffer, bytes_to_read, bytes_read);
        }

        let Some(player) = PLAYER.get() else {
            if !bytes_read.is_null() {
                unsafe {
                    *bytes_read = 0;
                }
            }
            return FALSE;
        };

        let mut guard = match player.lock() {
            Ok(guard) => guard,
            Err(_) => return FALSE,
        };

        let output =
            unsafe { slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read as usize) };
        match guard.read_next_blocking(output) {
            Ok(count) => {
                let read_index = REPLAY_READ_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_high_frequency(read_index) {
                    debug_line(&format!(
                        "[crosspuck] replay read #{} requested={} returned={}",
                        read_index, bytes_to_read, count
                    ));
                }
                if !bytes_read.is_null() {
                    unsafe {
                        *bytes_read = count as u32;
                    }
                }
                TRUE
            }
            Err(error) => {
                debug_line(&format!("[crosspuck] replay read failed: {error}"));
                if !bytes_read.is_null() {
                    unsafe {
                        *bytes_read = 0;
                    }
                }
                FALSE
            }
        }
    }

    fn read_virtual_hid_report_ready(
        profile: VirtualHidProfile,
        buffer: *mut c_void,
        bytes_to_read: u32,
    ) -> Result<Option<usize>, ()> {
        if buffer.is_null() {
            return Err(());
        }

        if !profile.is_active_controller_slot() {
            return Ok(None);
        }

        let mut bytes_read = 0_u32;
        if let Some(result) =
            read_pending_input_report(profile, buffer, bytes_to_read, &mut bytes_read as *mut u32)
        {
            return if result == TRUE {
                Ok(Some(bytes_read as usize))
            } else {
                Err(())
            };
        }

        if !replay_stream_active() {
            return read_recognition_report_ready(buffer, bytes_to_read);
        }

        let Some(player) = PLAYER.get() else {
            return Err(());
        };

        read_player_report_ready("replay", player, buffer, bytes_to_read)
    }

    fn read_pending_input_report(
        profile: VirtualHidProfile,
        buffer: *mut c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
    ) -> Option<BOOL> {
        let queue = PENDING_INPUT_REPORTS.get()?;
        let device = profile.sdl_device() as usize;
        let packet = {
            let mut guard = queue.lock().ok()?;
            let index = guard.iter().position(|pending| pending.device == device)?;
            guard.remove(index)?.report
        };
        let output =
            unsafe { slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read as usize) };
        output.fill(0);
        let count = output.len().min(packet.len());
        output[..count].copy_from_slice(&packet[..count]);

        let read_index = REPLAY_READ_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        debug_line(&format!(
            "[crosspuck] pending input read #{} requested={} returned={} head={}",
            read_index,
            bytes_to_read,
            count,
            unsafe { hex_head(buffer as *const u8, count.min(16) as u32) }
        ));

        if !bytes_read.is_null() {
            unsafe {
                *bytes_read = count as u32;
            }
        }

        Some(TRUE)
    }

    fn read_recognition_report(
        buffer: *mut c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
    ) -> BOOL {
        let Some(player) = RECOGNITION_PLAYER.get() else {
            if !bytes_read.is_null() {
                unsafe {
                    *bytes_read = 0;
                }
            }
            return FALSE;
        };

        let mut guard = match player.lock() {
            Ok(guard) => guard,
            Err(_) => return FALSE,
        };

        let output =
            unsafe { slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read as usize) };
        match guard.read_next_blocking(output) {
            Ok(count) => {
                let read_index = REPLAY_READ_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_high_frequency(read_index) {
                    debug_line(&format!(
                        "[crosspuck] recognition read #{} requested={} returned={}",
                        read_index, bytes_to_read, count
                    ));
                }
                if !bytes_read.is_null() {
                    unsafe {
                        *bytes_read = count as u32;
                    }
                }
                TRUE
            }
            Err(error) => {
                debug_line(&format!("[crosspuck] recognition read failed: {error}"));
                if !bytes_read.is_null() {
                    unsafe {
                        *bytes_read = 0;
                    }
                }
                FALSE
            }
        }
    }

    fn read_recognition_report_ready(
        buffer: *mut c_void,
        bytes_to_read: u32,
    ) -> Result<Option<usize>, ()> {
        let Some(player) = RECOGNITION_PLAYER.get() else {
            return Err(());
        };

        read_player_report_ready("recognition", player, buffer, bytes_to_read)
    }

    fn read_player_report_ready(
        label: &str,
        player: &Mutex<ReplayPlayer>,
        buffer: *mut c_void,
        bytes_to_read: u32,
    ) -> Result<Option<usize>, ()> {
        let mut guard = player.lock().map_err(|_| ())?;
        let output =
            unsafe { slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read as usize) };
        match guard.read_next_ready(output) {
            Ok(Some(count)) => {
                let read_index = REPLAY_READ_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_high_frequency(read_index) {
                    debug_line(&format!(
                        "[crosspuck] {label} ready read #{} requested={} returned={}",
                        read_index, bytes_to_read, count
                    ));
                }
                Ok(Some(count))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                debug_line(&format!("[crosspuck] {label} ready read failed: {error}"));
                Err(())
            }
        }
    }

    fn replay_stream_active() -> bool {
        let Some(config) = CONFIG.get() else {
            return false;
        };
        if !config.replay_enabled {
            return false;
        }
        if REPLAY_TAKEOVER_ACTIVE.load(Ordering::Relaxed) {
            return true;
        }

        let Some(player) = PLAYER.get() else {
            return false;
        };
        let elapsed = player
            .lock()
            .is_ok_and(|guard| guard.startup_delay_elapsed());
        if elapsed && !REPLAY_TAKEOVER_ACTIVE.swap(true, Ordering::Relaxed) {
            debug_line(&format!(
                "[crosspuck] replay takeover active after {}ms; recognition stream remains disabled for active slot",
                config.replay_delay.as_millis()
            ));
        }
        elapsed
    }

    fn should_claim_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        let Some(config) = CONFIG.get() else {
            return is_valve_puck_path(&lower) || is_default_wine_hid_path(&lower);
        };

        if config.claim_all_hid && lower.contains("hid") {
            return true;
        }

        if let Some(substr) = &config.claim_path_substr {
            return lower.contains(substr);
        }

        if is_valve_puck_path(&lower) {
            return true;
        }

        config.masquerade_wine_hid
            && lower.contains("vid_845e")
            && config
                .masquerade_wine_pids
                .iter()
                .any(|pid| lower.contains(&format!("pid_{pid}")))
    }

    fn is_valve_puck_path(lower_path: &str) -> bool {
        lower_path.contains("vid_28de") && lower_path.contains("pid_1304")
    }

    fn is_default_wine_hid_path(lower_path: &str) -> bool {
        lower_path.contains("vid_845e")
            && (lower_path.contains("pid_0002") || lower_path.contains("pid_0001"))
    }

    unsafe fn wide_z_to_string(ptr: PCWSTR) -> Option<String> {
        if ptr.is_null() {
            return None;
        }

        let mut len = 0;
        while len < 32_768 && *ptr.add(len) != 0 {
            len += 1;
        }

        Some(String::from_utf16_lossy(slice::from_raw_parts(ptr, len)))
    }

    unsafe fn narrow_z_to_string(ptr: PCSTR) -> Option<String> {
        if ptr.is_null() {
            return None;
        }

        let mut len = 0;
        while len < 32_768 && *ptr.add(len) != 0 {
            len += 1;
        }

        Some(String::from_utf8_lossy(slice::from_raw_parts(ptr, len)).to_string())
    }

    unsafe fn write_wide_string(buffer: *mut c_void, buffer_len: u32, value: &str) -> u8 {
        if buffer.is_null() || buffer_len < 2 {
            return FALSE_U8;
        }

        let slots = buffer_len as usize / 2;
        let out = slice::from_raw_parts_mut(buffer as *mut u16, slots);
        let mut written = 0;
        for unit in value.encode_utf16().take(slots.saturating_sub(1)) {
            out[written] = unit;
            written += 1;
        }
        out[written] = 0;
        TRUE_U8
    }

    unsafe fn zero_buffer(buffer: *mut c_void, buffer_len: u32) {
        if !buffer.is_null() && buffer_len > 0 {
            ptr::write_bytes(buffer, 0, buffer_len as usize);
        }
    }

    unsafe fn zero_buffer_with_report_id(buffer: *mut c_void, buffer_len: u32, report_id: u8) {
        zero_buffer(buffer, buffer_len);
        if !buffer.is_null() && buffer_len > 0 {
            *(buffer as *mut u8) = report_id;
        }
    }

    unsafe fn report_id_from_buffer(buffer: *const u8, buffer_len: u32) -> u8 {
        if buffer.is_null() || buffer_len == 0 {
            0
        } else {
            *buffer
        }
    }

    fn bool_to_u8(value: BOOL) -> u8 {
        if value == TRUE {
            TRUE_U8
        } else {
            FALSE_U8
        }
    }

    fn is_virtual_handle(handle: HANDLE) -> bool {
        virtual_profile_for_handle(handle).is_some()
    }

    fn invalid_handle_value() -> HANDLE {
        (-1_isize) as HANDLE
    }

    fn is_valid_real_handle(handle: HANDLE) -> bool {
        !handle.is_null() && handle != invalid_handle_value()
    }

    fn virtual_profile_for_path(path: &str) -> VirtualHidProfile {
        let lower = path.to_ascii_lowercase();
        if lower.contains("pid_0002")
            || lower.contains("mi_06")
            || (lower.contains("vid_28de") && lower.contains("pid_1304") && lower.contains("col02"))
        {
            VirtualHidProfile::VendorDongle
        } else if lower.contains("mi_03") {
            VirtualHidProfile::Interface3
        } else if lower.contains("mi_04") {
            VirtualHidProfile::Interface4
        } else if lower.contains("mi_05") {
            VirtualHidProfile::Interface5
        } else {
            VirtualHidProfile::Main
        }
    }

    fn virtual_profile_for_handle(handle: HANDLE) -> Option<VirtualHidProfile> {
        if handle == VirtualHidProfile::Main.handle() {
            Some(VirtualHidProfile::Main)
        } else if handle == VirtualHidProfile::Interface3.handle() {
            Some(VirtualHidProfile::Interface3)
        } else if handle == VirtualHidProfile::Interface4.handle() {
            Some(VirtualHidProfile::Interface4)
        } else if handle == VirtualHidProfile::Interface5.handle() {
            Some(VirtualHidProfile::Interface5)
        } else if handle == VirtualHidProfile::VendorDongle.handle() {
            Some(VirtualHidProfile::VendorDongle)
        } else {
            None
        }
    }

    fn mark_virtual_profile_open(profile: VirtualHidProfile) -> usize {
        virtual_open_count(profile).fetch_add(1, Ordering::Relaxed) + 1
    }

    fn mark_virtual_handle_closed(handle: HANDLE) -> usize {
        let Some(profile) = virtual_profile_for_handle(handle) else {
            return 0;
        };
        let counter = virtual_open_count(profile);
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_sub(1))
            })
            .map(|previous| previous.saturating_sub(1))
            .unwrap_or(0)
    }

    fn virtual_handle_is_open(handle: HANDLE) -> bool {
        let Some(profile) = virtual_profile_for_handle(handle) else {
            return false;
        };
        virtual_open_count(profile).load(Ordering::Relaxed) > 0
    }

    fn virtual_stream_connected() -> bool {
        VIRTUAL_STREAM_CONNECTED.load(Ordering::Relaxed)
    }

    fn active_virtual_session() -> bool {
        virtual_profiles_open_count() > 0 || REPLAY_READ_COUNT.load(Ordering::Relaxed) > 0
    }

    fn disconnect_virtual_stream(reason: &str) {
        if VIRTUAL_STREAM_CONNECTED.swap(false, Ordering::Relaxed) {
            debug_line(&format!(
                "[crosspuck] virtual stream disconnected reason={reason}"
            ));
        }
    }

    fn disconnect_virtual_stream_quiet() {
        VIRTUAL_STREAM_CONNECTED.store(false, Ordering::Relaxed);
    }

    fn virtual_open_count(profile: VirtualHidProfile) -> &'static AtomicUsize {
        match profile {
            VirtualHidProfile::Main => &MAIN_OPEN_COUNT,
            VirtualHidProfile::Interface3 => &IF3_OPEN_COUNT,
            VirtualHidProfile::Interface4 => &IF4_OPEN_COUNT,
            VirtualHidProfile::Interface5 => &IF5_OPEN_COUNT,
            VirtualHidProfile::VendorDongle => &VENDOR_OPEN_COUNT,
        }
    }

    fn virtual_profiles_open_count() -> usize {
        MAIN_OPEN_COUNT.load(Ordering::Relaxed)
            + IF3_OPEN_COUNT.load(Ordering::Relaxed)
            + IF4_OPEN_COUNT.load(Ordering::Relaxed)
            + IF5_OPEN_COUNT.load(Ordering::Relaxed)
            + VENDOR_OPEN_COUNT.load(Ordering::Relaxed)
    }

    fn virtual_profile_for_preparsed_data(
        preparsed_data: *mut c_void,
    ) -> Option<VirtualHidProfile> {
        if preparsed_data == VirtualHidProfile::Main.preparsed_data() {
            Some(VirtualHidProfile::Main)
        } else if preparsed_data == VirtualHidProfile::Interface3.preparsed_data() {
            Some(VirtualHidProfile::Interface3)
        } else if preparsed_data == VirtualHidProfile::Interface4.preparsed_data() {
            Some(VirtualHidProfile::Interface4)
        } else if preparsed_data == VirtualHidProfile::Interface5.preparsed_data() {
            Some(VirtualHidProfile::Interface5)
        } else if preparsed_data == VirtualHidProfile::VendorDongle.preparsed_data() {
            Some(VirtualHidProfile::VendorDongle)
        } else {
            None
        }
    }

    fn virtual_profile_from_text(text: &str) -> Option<VirtualHidProfile> {
        let lower = text.to_ascii_lowercase();
        if lower.contains("vid_845e&pid_0002")
            || lower.contains("vid_28de&pid_1304&mi_06")
            || lower.contains("vid_28de&pid_1304") && lower.contains("col02")
            || lower.contains("hid_device_up:ff00_u:0002")
            || lower.contains("up:ff00_u:0002")
        {
            Some(VirtualHidProfile::VendorDongle)
        } else if lower.contains("vid_28de&pid_1304&mi_03") {
            Some(VirtualHidProfile::Interface3)
        } else if lower.contains("vid_28de&pid_1304&mi_04") {
            Some(VirtualHidProfile::Interface4)
        } else if lower.contains("vid_28de&pid_1304&mi_05") {
            Some(VirtualHidProfile::Interface5)
        } else if lower.contains("vid_845e&pid_0001")
            || lower.contains("vid_28de&pid_1304&mi_02")
            || lower.contains("hid_device_up:0001_u:0001")
            || lower.contains("up:0001_u:0001")
        {
            Some(VirtualHidProfile::Main)
        } else {
            None
        }
    }

    unsafe fn devinst_from_device_info_data(device_info_data: *mut c_void) -> Option<u32> {
        if device_info_data.is_null() {
            return None;
        }

        let data = &*(device_info_data as *const SpDevinfoData);
        if data.cb_size < 24 {
            return None;
        }
        Some(data.dev_inst)
    }

    unsafe fn virtual_profile_for_device_info_data(
        device_info_data: *mut c_void,
    ) -> Option<VirtualHidProfile> {
        let devinst = devinst_from_device_info_data(device_info_data)?;
        virtual_profile_for_devinst(devinst)
    }

    fn virtual_profile_for_devinst(devinst: u32) -> Option<VirtualHidProfile> {
        let guard = VIRTUAL_DEVINSTS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .ok()?;
        guard
            .iter()
            .find_map(|(stored_devinst, profile)| (*stored_devinst == devinst).then_some(*profile))
    }

    unsafe fn remember_virtual_device_info(
        device_info_data: *mut c_void,
        profile: VirtualHidProfile,
        source: &str,
    ) {
        let Some(devinst) = devinst_from_device_info_data(device_info_data) else {
            return;
        };
        remember_virtual_devinst(devinst, profile, source);
    }

    fn remember_virtual_devinst(devinst: u32, profile: VirtualHidProfile, source: &str) {
        let mut guard = match VIRTUAL_DEVINSTS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
        {
            Ok(guard) => guard,
            Err(_) => return,
        };

        if let Some((_, stored_profile)) = guard
            .iter_mut()
            .find(|(stored_devinst, _)| *stored_devinst == devinst)
        {
            if *stored_profile != profile {
                debug_line(&format!(
                    "[crosspuck] remap devinst=0x{devinst:08X} {} -> {} via {source}",
                    stored_profile.label(),
                    profile.label()
                ));
                *stored_profile = profile;
            }
            return;
        }

        guard.push((devinst, profile));
        debug_line(&format!(
            "[crosspuck] remember devinst=0x{devinst:08X} profile={} via {source}",
            profile.label()
        ));
    }

    fn device_instance_id_value(profile: VirtualHidProfile) -> &'static str {
        match profile {
            VirtualHidProfile::Main => r"HID\VID_28DE&PID_1304&MI_02&COL01\FXB9961303C9C&0&0001",
            VirtualHidProfile::Interface3 => {
                r"HID\VID_28DE&PID_1304&MI_03&COL01\FXB9961303C9C&0&0002"
            }
            VirtualHidProfile::Interface4 => {
                r"HID\VID_28DE&PID_1304&MI_04&COL01\FXB9961303C9C&0&0003"
            }
            VirtualHidProfile::Interface5 => {
                r"HID\VID_28DE&PID_1304&MI_05&COL01\FXB9961303C9C&0&0004"
            }
            VirtualHidProfile::VendorDongle => {
                r"HID\VID_28DE&PID_1304&MI_06&COL02\FXB9961303C9C&0&0005"
            }
        }
    }

    fn hardware_id_entries(profile: VirtualHidProfile) -> Vec<&'static str> {
        match profile {
            VirtualHidProfile::Main => vec![
                r"HID\VID_28DE&PID_1304&MI_02&COL01",
                r"HID\VID_28DE&PID_1304&MI_02",
                r"HID\VID_28DE&PID_1304",
                r"HID\VID_28DE&UP:0001_U:0001",
                r"HID_DEVICE_UP:0001_U:0001",
                r"HID_DEVICE",
            ],
            VirtualHidProfile::Interface3 => vec![
                r"HID\VID_28DE&PID_1304&MI_03&COL01",
                r"HID\VID_28DE&PID_1304&MI_03",
                r"HID\VID_28DE&PID_1304",
                r"HID\VID_28DE&UP:0001_U:0001",
                r"HID_DEVICE_UP:0001_U:0001",
                r"HID_DEVICE",
            ],
            VirtualHidProfile::Interface4 => vec![
                r"HID\VID_28DE&PID_1304&MI_04&COL01",
                r"HID\VID_28DE&PID_1304&MI_04",
                r"HID\VID_28DE&PID_1304",
                r"HID\VID_28DE&UP:0001_U:0001",
                r"HID_DEVICE_UP:0001_U:0001",
                r"HID_DEVICE",
            ],
            VirtualHidProfile::Interface5 => vec![
                r"HID\VID_28DE&PID_1304&MI_05&COL01",
                r"HID\VID_28DE&PID_1304&MI_05",
                r"HID\VID_28DE&PID_1304",
                r"HID\VID_28DE&UP:0001_U:0001",
                r"HID_DEVICE_UP:0001_U:0001",
                r"HID_DEVICE",
            ],
            VirtualHidProfile::VendorDongle => vec![
                r"HID\VID_28DE&PID_1304&MI_06&COL02",
                r"HID\VID_28DE&PID_1304&MI_06",
                r"HID\VID_28DE&PID_1304",
                r"HID\VID_28DE&UP:FF00_U:0002",
                r"HID_DEVICE_UP:FF00_U:0002",
                r"HID_DEVICE",
            ],
        }
    }

    fn compatible_id_entries(profile: VirtualHidProfile) -> Vec<&'static str> {
        if profile == VirtualHidProfile::VendorDongle {
            vec![r"HID_DEVICE_UP:FF00_U:0002", r"HID_DEVICE"]
        } else {
            vec![r"HID_DEVICE_UP:0001_U:0001", r"HID_DEVICE"]
        }
    }

    fn location_path_entry(profile: VirtualHidProfile) -> &'static str {
        match profile {
            VirtualHidProfile::Main => r"USBROOT(0)#USB(1)#USBMI(2)#HID(1)",
            VirtualHidProfile::Interface3 => r"USBROOT(0)#USB(1)#USBMI(3)#HID(1)",
            VirtualHidProfile::Interface4 => r"USBROOT(0)#USB(1)#USBMI(4)#HID(1)",
            VirtualHidProfile::Interface5 => r"USBROOT(0)#USB(1)#USBMI(5)#HID(1)",
            VirtualHidProfile::VendorDongle => r"USBROOT(0)#USB(1)#USBMI(6)#HID(1)",
        }
    }

    fn registry_property_value(profile: VirtualHidProfile, property: u32) -> Option<RegistryValue> {
        let value = match property {
            SPDRP_DEVICEDESC | SPDRP_FRIENDLYNAME => RegistryValue {
                reg_type: REG_SZ,
                entries: vec![VIRTUAL_PRODUCT],
            },
            SPDRP_MFG => RegistryValue {
                reg_type: REG_SZ,
                entries: vec![VIRTUAL_MANUFACTURER],
            },
            SPDRP_SERVICE => RegistryValue {
                reg_type: REG_SZ,
                entries: vec!["HidUsb"],
            },
            SPDRP_CLASS => RegistryValue {
                reg_type: REG_SZ,
                entries: vec!["HIDClass"],
            },
            SPDRP_ENUMERATOR_NAME => RegistryValue {
                reg_type: REG_SZ,
                entries: vec!["HID"],
            },
            SPDRP_HARDWAREID => RegistryValue {
                reg_type: REG_MULTI_SZ,
                entries: hardware_id_entries(profile),
            },
            SPDRP_COMPATIBLEIDS => RegistryValue {
                reg_type: REG_MULTI_SZ,
                entries: compatible_id_entries(profile),
            },
            SPDRP_LOCATION_PATHS => RegistryValue {
                reg_type: REG_MULTI_SZ,
                entries: vec![location_path_entry(profile)],
            },
            _ => return None,
        };
        Some(value)
    }

    fn device_property_value(
        profile: VirtualHidProfile,
        property_key: *const DEVPROPKEY,
    ) -> Option<DevicePropertyValue> {
        let key = unsafe { property_key.as_ref()? };
        let value = if devpropkey_eq(key, &DEVPKEY_DEVICE_DEVICE_DESC)
            || devpropkey_eq(key, &DEVPKEY_DEVICE_FRIENDLY_NAME)
            || devpropkey_eq(key, &DEVPKEY_DEVICE_BUS_REPORTED_DEVICE_DESC)
        {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING,
                entries: vec![VIRTUAL_PRODUCT],
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_MANUFACTURER) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING,
                entries: vec![VIRTUAL_MANUFACTURER],
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_INSTANCE_ID) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING,
                entries: vec![device_instance_id_value(profile)],
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_SERVICE) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING,
                entries: vec!["HidUsb"],
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_CLASS) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING,
                entries: vec!["HIDClass"],
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_ENUMERATOR_NAME) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING,
                entries: vec!["HID"],
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_HARDWARE_IDS) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING_LIST,
                entries: hardware_id_entries(profile),
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_COMPATIBLE_IDS) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING_LIST,
                entries: compatible_id_entries(profile),
            }
        } else if devpropkey_eq(key, &DEVPKEY_DEVICE_LOCATION_PATHS) {
            DevicePropertyValue {
                prop_type: DEVPROP_TYPE_STRING_LIST,
                entries: vec![location_path_entry(profile)],
            }
        } else {
            return None;
        };
        Some(value)
    }

    unsafe fn registry_w_text(
        buffer: *const u8,
        buffer_size: u32,
        required_size: *const u32,
    ) -> Option<String> {
        let byte_len = registry_actual_byte_len(buffer_size, required_size);
        if buffer.is_null() || byte_len < 2 {
            return None;
        }
        let units = slice::from_raw_parts(buffer as *const u16, byte_len / 2);
        Some(decode_wide_registry_units(units))
    }

    unsafe fn registry_a_text(
        buffer: *const u8,
        buffer_size: u32,
        required_size: *const u32,
    ) -> Option<String> {
        let byte_len = registry_actual_byte_len(buffer_size, required_size);
        if buffer.is_null() || byte_len == 0 {
            return None;
        }
        let bytes = slice::from_raw_parts(buffer, byte_len);
        Some(decode_narrow_registry_bytes(bytes))
    }

    unsafe fn write_registry_value_w(
        buffer: *mut u8,
        buffer_size: u32,
        required_size: *mut u32,
        value: &RegistryValue,
    ) -> bool {
        let mut units = Vec::new();
        if value.reg_type == REG_MULTI_SZ {
            for entry in &value.entries {
                units.extend(entry.encode_utf16());
                units.push(0);
            }
            units.push(0);
        } else {
            units.extend(
                value
                    .entries
                    .first()
                    .copied()
                    .unwrap_or_default()
                    .encode_utf16(),
            );
            units.push(0);
        }

        let byte_len = units.len() * 2;
        if !required_size.is_null() {
            *required_size = byte_len as u32;
        }
        if buffer.is_null() || byte_len > buffer_size as usize {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return false;
        }

        ptr::copy_nonoverlapping(units.as_ptr() as *const u8, buffer, byte_len);
        true
    }

    unsafe fn write_device_property_value_w(
        buffer: *mut u8,
        buffer_size: u32,
        required_size: *mut u32,
        value: &DevicePropertyValue,
    ) -> bool {
        let mut units = Vec::new();
        if value.prop_type == DEVPROP_TYPE_STRING_LIST {
            for entry in &value.entries {
                units.extend(entry.encode_utf16());
                units.push(0);
            }
            units.push(0);
        } else {
            units.extend(
                value
                    .entries
                    .first()
                    .copied()
                    .unwrap_or_default()
                    .encode_utf16(),
            );
            units.push(0);
        }

        let byte_len = units.len() * 2;
        if !required_size.is_null() {
            *required_size = byte_len as u32;
        }
        if buffer.is_null() || byte_len > buffer_size as usize {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return false;
        }

        ptr::copy_nonoverlapping(units.as_ptr() as *const u8, buffer, byte_len);
        true
    }

    unsafe fn write_registry_value_a(
        buffer: *mut u8,
        buffer_size: u32,
        required_size: *mut u32,
        value: &RegistryValue,
    ) -> bool {
        let mut bytes = Vec::new();
        if value.reg_type == REG_MULTI_SZ {
            for entry in &value.entries {
                bytes.extend_from_slice(entry.as_bytes());
                bytes.push(0);
            }
            bytes.push(0);
        } else {
            bytes.extend_from_slice(
                value
                    .entries
                    .first()
                    .copied()
                    .unwrap_or_default()
                    .as_bytes(),
            );
            bytes.push(0);
        }

        if !required_size.is_null() {
            *required_size = bytes.len() as u32;
        }
        if buffer.is_null() || bytes.len() > buffer_size as usize {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return false;
        }

        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        true
    }

    unsafe fn write_instance_id_a(
        buffer: *mut u8,
        buffer_size_chars: u32,
        required_size: *mut u32,
        value: &str,
    ) -> bool {
        let bytes = value.as_bytes();
        let required = bytes.len() + 1;
        if !required_size.is_null() {
            *required_size = required as u32;
        }
        if buffer.is_null() || required > buffer_size_chars as usize {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return false;
        }

        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        *buffer.add(bytes.len()) = 0;
        true
    }

    unsafe fn write_instance_id_w(
        buffer: *mut u16,
        buffer_size_chars: u32,
        required_size: *mut u32,
        value: &str,
    ) -> bool {
        let units: Vec<u16> = value.encode_utf16().collect();
        let required = units.len() + 1;
        if !required_size.is_null() {
            *required_size = required as u32;
        }
        if buffer.is_null() || required > buffer_size_chars as usize {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return false;
        }

        ptr::copy_nonoverlapping(units.as_ptr(), buffer, units.len());
        *buffer.add(units.len()) = 0;
        true
    }

    fn registry_actual_byte_len(buffer_size: u32, required_size: *const u32) -> usize {
        if required_size.is_null() {
            return buffer_size as usize;
        }

        let required = unsafe { *required_size as usize };
        if required == 0 {
            buffer_size as usize
        } else {
            required.min(buffer_size as usize)
        }
    }

    fn decode_wide_registry_units(units: &[u16]) -> String {
        let mut parts = Vec::new();
        let mut start = 0;
        for (index, unit) in units.iter().copied().enumerate() {
            if unit == 0 {
                if index == start {
                    break;
                }
                parts.push(String::from_utf16_lossy(&units[start..index]));
                start = index + 1;
            }
        }

        if parts.is_empty() {
            String::from_utf16_lossy(units)
                .trim_matches(char::from(0))
                .to_string()
        } else {
            parts.join("|")
        }
    }

    fn decode_narrow_registry_bytes(bytes: &[u8]) -> String {
        let mut parts = Vec::new();
        let mut start = 0;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if byte == 0 {
                if index == start {
                    break;
                }
                parts.push(String::from_utf8_lossy(&bytes[start..index]).to_string());
                start = index + 1;
            }
        }

        if parts.is_empty() {
            String::from_utf8_lossy(bytes)
                .trim_matches(char::from(0))
                .to_string()
        } else {
            parts.join("|")
        }
    }

    fn required_value_size_w(value: &RegistryValue) -> usize {
        if value.reg_type == REG_MULTI_SZ {
            (value
                .entries
                .iter()
                .map(|entry| entry.encode_utf16().count() + 1)
                .sum::<usize>()
                + 1)
                * 2
        } else {
            (value
                .entries
                .first()
                .copied()
                .unwrap_or_default()
                .encode_utf16()
                .count()
                + 1)
                * 2
        }
    }

    fn required_value_size_a(value: &RegistryValue) -> usize {
        if value.reg_type == REG_MULTI_SZ {
            value
                .entries
                .iter()
                .map(|entry| entry.len() + 1)
                .sum::<usize>()
                + 1
        } else {
            value.entries.first().copied().unwrap_or_default().len() + 1
        }
    }

    fn required_device_property_size_w(value: &DevicePropertyValue) -> usize {
        if value.prop_type == DEVPROP_TYPE_STRING_LIST {
            (value
                .entries
                .iter()
                .map(|entry| entry.encode_utf16().count() + 1)
                .sum::<usize>()
                + 1)
                * 2
        } else {
            (value
                .entries
                .first()
                .copied()
                .unwrap_or_default()
                .encode_utf16()
                .count()
                + 1)
                * 2
        }
    }

    fn registry_property_label(property: u32) -> &'static str {
        match property {
            SPDRP_DEVICEDESC => "SPDRP_DEVICEDESC",
            SPDRP_HARDWAREID => "SPDRP_HARDWAREID",
            SPDRP_COMPATIBLEIDS => "SPDRP_COMPATIBLEIDS",
            SPDRP_SERVICE => "SPDRP_SERVICE",
            SPDRP_CLASS => "SPDRP_CLASS",
            SPDRP_MFG => "SPDRP_MFG",
            SPDRP_FRIENDLYNAME => "SPDRP_FRIENDLYNAME",
            SPDRP_ENUMERATOR_NAME => "SPDRP_ENUMERATOR_NAME",
            SPDRP_LOCATION_PATHS => "SPDRP_LOCATION_PATHS",
            _ => "SPDRP_UNKNOWN",
        }
    }

    fn registry_type_label(reg_type: u32) -> &'static str {
        match reg_type {
            REG_SZ => "REG_SZ",
            REG_MULTI_SZ => "REG_MULTI_SZ",
            _ => "REG_UNKNOWN",
        }
    }

    fn devprop_type_label(prop_type: u32) -> &'static str {
        match prop_type {
            DEVPROP_TYPE_STRING => "DEVPROP_TYPE_STRING",
            DEVPROP_TYPE_STRING_LIST => "DEVPROP_TYPE_STRING_LIST",
            _ => "DEVPROP_TYPE_UNKNOWN",
        }
    }

    fn devpropkey_eq(left: &DEVPROPKEY, right: &DEVPROPKEY) -> bool {
        left.pid == right.pid
            && left.fmtid.data1 == right.fmtid.data1
            && left.fmtid.data2 == right.fmtid.data2
            && left.fmtid.data3 == right.fmtid.data3
            && left.fmtid.data4 == right.fmtid.data4
    }

    fn devpropkey_label(property_key: *const DEVPROPKEY) -> String {
        let Some(key) = (unsafe { property_key.as_ref() }) else {
            return "null".to_string();
        };

        let known = [
            (&DEVPKEY_DEVICE_DEVICE_DESC, "DEVPKEY_Device_DeviceDesc"),
            (&DEVPKEY_DEVICE_HARDWARE_IDS, "DEVPKEY_Device_HardwareIds"),
            (
                &DEVPKEY_DEVICE_COMPATIBLE_IDS,
                "DEVPKEY_Device_CompatibleIds",
            ),
            (&DEVPKEY_DEVICE_SERVICE, "DEVPKEY_Device_Service"),
            (&DEVPKEY_DEVICE_CLASS, "DEVPKEY_Device_Class"),
            (&DEVPKEY_DEVICE_MANUFACTURER, "DEVPKEY_Device_Manufacturer"),
            (&DEVPKEY_DEVICE_FRIENDLY_NAME, "DEVPKEY_Device_FriendlyName"),
            (
                &DEVPKEY_DEVICE_ENUMERATOR_NAME,
                "DEVPKEY_Device_EnumeratorName",
            ),
            (
                &DEVPKEY_DEVICE_LOCATION_PATHS,
                "DEVPKEY_Device_LocationPaths",
            ),
            (
                &DEVPKEY_DEVICE_BUS_REPORTED_DEVICE_DESC,
                "DEVPKEY_Device_BusReportedDeviceDesc",
            ),
            (&DEVPKEY_DEVICE_INSTANCE_ID, "DEVPKEY_Device_InstanceId"),
        ];
        for (known_key, label) in known {
            if devpropkey_eq(key, known_key) {
                return label.to_string();
            }
        }

        format!("{} pid={}", guid_value_label(&key.fmtid), key.pid)
    }

    unsafe fn device_property_w_text(
        buffer: *const u8,
        buffer_size: u32,
        required_size: *const u32,
        prop_type: u32,
    ) -> Option<String> {
        if prop_type != DEVPROP_TYPE_STRING && prop_type != DEVPROP_TYPE_STRING_LIST {
            return None;
        }
        registry_w_text(buffer, buffer_size, required_size)
    }

    fn log_registry_property_result(
        api: &str,
        property: u32,
        result: BOOL,
        existing: Option<&str>,
    ) {
        if result != TRUE && existing.is_none() {
            return;
        }

        let interesting = existing.map(looks_interesting_path).unwrap_or(matches!(
            property,
            SPDRP_DEVICEDESC
                | SPDRP_HARDWAREID
                | SPDRP_COMPATIBLEIDS
                | SPDRP_FRIENDLYNAME
                | SPDRP_MFG
        ));
        if interesting {
            debug_line(&format!(
                "[crosspuck] {api} property={} -> {} value={:?}",
                registry_property_label(property),
                bool_label(result),
                existing.unwrap_or("")
            ));
        }
    }

    fn log_device_property_result(
        api: &str,
        property_key: *const DEVPROPKEY,
        prop_type: u32,
        result: BOOL,
        existing: Option<&str>,
    ) {
        let key_label = devpropkey_label(property_key);
        let interesting = existing.map(looks_interesting_path).unwrap_or_else(|| {
            key_label.contains("DeviceDesc")
                || key_label.contains("HardwareIds")
                || key_label.contains("CompatibleIds")
                || key_label.contains("FriendlyName")
                || key_label.contains("Manufacturer")
                || key_label.contains("InstanceId")
                || key_label.contains("BusReportedDeviceDesc")
        });
        if interesting || result == FALSE {
            debug_line(&format!(
                "[crosspuck] {api} key={} type={} -> {} value={:?}",
                key_label,
                devprop_type_label(prop_type),
                bool_label(result),
                existing.unwrap_or("")
            ));
        }
    }

    fn env_u64(name: &str, default: u64) -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(default)
    }

    fn env_bool(name: &str, default: bool) -> bool {
        std::env::var(name)
            .map(|value| {
                value == "1"
                    || value.eq_ignore_ascii_case("true")
                    || value.eq_ignore_ascii_case("yes")
                    || value.eq_ignore_ascii_case("on")
            })
            .unwrap_or(default)
    }

    fn initialize_log_path(hinst_value: usize) {
        let directory = dll_directory(hinst_value)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let path = directory.join("a.txt");
        let _ = LOG_PATH.set(path.clone());

        let cwd = std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| format!("<unavailable: {error}>"));
        let pid = unsafe { GetCurrentProcessId() };
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let dll_path = module_file_path(hinst_value as HINSTANCE)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unavailable>".to_string());
        let exe_path = module_file_path(ptr::null_mut())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unavailable>".to_string());
        let message = format!(
            "\n==== crosspuck hid.dll loaded pid={pid} unix_ms={timestamp_ms} ====\nmarker=a.txt\n[crosspuck] diagnostic log start\n[crosspuck] pid={pid}\n[crosspuck] exe={exe_path}\n[crosspuck] dll={dll_path}\n[crosspuck] dll_dir={}\n[crosspuck] cwd={cwd}\n",
            directory.display()
        );
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(message.as_bytes()));
    }

    fn dll_directory(hinst_value: usize) -> Option<PathBuf> {
        module_file_path(hinst_value as HINSTANCE)
            .and_then(|dll_path| dll_path.parent().map(PathBuf::from))
    }

    fn module_file_path(module: HINSTANCE) -> Option<PathBuf> {
        let mut buffer = vec![0_u16; 32_768];
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
        if len == 0 {
            return None;
        }

        buffer.truncate(len as usize);
        Some(PathBuf::from(String::from_utf16_lossy(&buffer)))
    }

    fn debug_line(message: &str) {
        let mut bytes = Vec::with_capacity(message.len() + 2);
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(b'\n');
        bytes.push(0);
        unsafe {
            OutputDebugStringA(bytes.as_ptr());
        }
        append_log_line(message);
    }

    fn append_log_line(message: &str) {
        let Some(path) = LOG_PATH.get() else {
            return;
        };

        LOGGING.with(|logging| {
            if logging.get() {
                return;
            }

            logging.set(true);
            let result = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| writeln!(file, "{message}"));
            logging.set(false);

            if result.is_err() {
                // Avoid recursively logging logging failures.
            }
        });
    }

    fn is_logging() -> bool {
        LOGGING.with(Cell::get)
    }

    fn log_virtual_io(message: &str) {
        let count = VIRTUAL_IO_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_high_frequency(count) {
            debug_line(message);
        }
    }

    fn log_hidd_report_call(message: &str) {
        let count = HIDD_REPORT_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_high_frequency(count) {
            debug_line(message);
        }
    }

    fn log_setupdi_line(message: &str) {
        let count = SETUPDI_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_high_frequency(count) {
            debug_line(message);
        }
    }

    unsafe fn trace_virtual_report(
        api: &str,
        phase: &str,
        handle: HANDLE,
        buffer: *const u8,
        buffer_len: u32,
    ) {
        let Some(config) = CONFIG.get() else {
            return;
        };
        if !config.trace_reports {
            return;
        }

        let count = REPORT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count > config.trace_report_limit {
            if count == config.trace_report_limit + 1 {
                debug_line(&format!(
                    "[crosspuck] report trace limit reached; further payload traces suppressed limit={}",
                    config.trace_report_limit
                ));
            }
            return;
        }

        let report_id = report_id_from_buffer(buffer, buffer_len);
        debug_line(&format!(
            "[crosspuck] trace report #{} {} {} handle={} len={} report_id=0x{:02X} bytes={}",
            count,
            api,
            phase,
            handle_label(handle),
            buffer_len,
            report_id,
            hex_bytes(buffer, buffer_len, config.trace_report_max_bytes)
        ));
    }

    fn should_log_high_frequency(count: usize) -> bool {
        count <= 8 || count.is_power_of_two() || count % 1_048_576 == 0
    }

    fn log_create_file_path(api: &str, path: &str) {
        if !looks_interesting_path(path) {
            return;
        }

        let count = CREATE_FILE_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count <= 256 || count % 512 == 0 {
            debug_line(&format!("[crosspuck] {api} pass path={path:?}"));
        }
    }

    fn remember_controller_log_handle(api: &str, handle: HANDLE, path: &str) {
        if !is_valid_real_handle(handle) {
            return;
        }

        let handle_value = handle as usize;
        let handles = CONTROLLER_LOG_HANDLES.get_or_init(|| Mutex::new(Vec::new()));
        let mut should_log = false;
        if let Ok(mut guard) = handles.lock() {
            if !guard.contains(&handle_value) {
                guard.push(handle_value);
                should_log = true;
            }
        }

        if should_log {
            debug_line(&format!(
                "[crosspuck] {api} tracking controller log handle={} path={path:?}",
                handle_label(handle)
            ));
        }
    }

    fn forget_controller_log_handle(handle: HANDLE) {
        if handle.is_null() {
            return;
        }

        let Some(handles) = CONTROLLER_LOG_HANDLES.get() else {
            return;
        };
        let handle_value = handle as usize;
        let mut removed = false;
        if let Ok(mut guard) = handles.lock() {
            let before = guard.len();
            guard.retain(|tracked| *tracked != handle_value);
            removed = guard.len() != before;
        }

        if removed {
            debug_line(&format!(
                "[crosspuck] controller log handle closed handle={}",
                handle_label(handle)
            ));
        }
    }

    fn is_controller_log_handle(handle: HANDLE) -> bool {
        let Some(handles) = CONTROLLER_LOG_HANDLES.get() else {
            return false;
        };
        let handle_value = handle as usize;
        handles
            .lock()
            .is_ok_and(|guard| guard.contains(&handle_value))
    }

    fn remember_sdl_virtual_hid_device(api: &str, device: *mut c_void, path: &str) {
        if device.is_null() {
            debug_line(&format!(
                "[crosspuck] {api} virtual HID open returned null path={path:?}"
            ));
            return;
        }

        let device_value = device as usize;
        let devices = SDL_VIRTUAL_HID_DEVICES.get_or_init(|| Mutex::new(Vec::new()));
        let mut should_log = false;
        if let Ok(mut guard) = devices.lock() {
            if !guard.contains(&device_value) {
                guard.push(device_value);
                should_log = true;
            }
        }

        if should_log {
            debug_line(&format!(
                "[crosspuck] {api} tracking SDL virtual HID device={device:p} path={path:?}"
            ));
        }
    }

    fn forget_sdl_virtual_hid_device(device: *mut c_void) {
        if device.is_null() {
            return;
        }

        let Some(devices) = SDL_VIRTUAL_HID_DEVICES.get() else {
            return;
        };
        let device_value = device as usize;
        let mut removed = false;
        if let Ok(mut guard) = devices.lock() {
            let before = guard.len();
            guard.retain(|tracked| *tracked != device_value);
            removed = guard.len() != before;
        }

        if removed {
            debug_line(&format!(
                "[crosspuck] SDL virtual HID device closed device={device:p}"
            ));
        }
    }

    fn is_sdl_virtual_hid_device(device: *mut c_void) -> bool {
        let Some(devices) = SDL_VIRTUAL_HID_DEVICES.get() else {
            return false;
        };
        let device_value = device as usize;
        devices
            .lock()
            .is_ok_and(|guard| guard.contains(&device_value))
    }

    fn virtual_profile_for_fake_sdl_device(device: *mut c_void) -> Option<VirtualHidProfile> {
        let value = device as HANDLE;
        virtual_profile_for_handle(value)
    }

    unsafe fn should_augment_sdl_hid_enumeration(vendor_id: u16, product_id: u16) -> bool {
        (vendor_id == 0 || vendor_id == VIRTUAL_VENDOR_ID)
            && (product_id == 0 || product_id == VIRTUAL_PRODUCT_ID)
    }

    unsafe fn augment_sdl_hid_enumeration(head: *mut SdlHidDeviceInfo) -> *mut SdlHidDeviceInfo {
        let mut seen_main = false;
        let mut seen_vendor = false;
        let mut tail = ptr::null_mut();
        let mut cursor = head;
        while !cursor.is_null() {
            tail = cursor;
            let path = narrow_z_to_string((*cursor).path as PCSTR).unwrap_or_default();
            if path.to_ascii_lowercase().contains("vid_845e&pid_0001") {
                rewrite_sdl_hid_device_info(cursor, VirtualHidProfile::Main);
                seen_main = true;
            } else if path.to_ascii_lowercase().contains("vid_845e&pid_0002") {
                rewrite_sdl_hid_device_info(cursor, VirtualHidProfile::VendorDongle);
                seen_vendor = true;
            } else if let Some(profile) = virtual_profile_from_text(&path) {
                if profile == VirtualHidProfile::Main {
                    seen_main = true;
                } else if profile == VirtualHidProfile::VendorDongle {
                    seen_vendor = true;
                }
            }
            cursor = (*cursor).next;
        }

        let mut missing_profiles = Vec::new();
        if !seen_main {
            missing_profiles.push(VirtualHidProfile::Main);
        }
        if !seen_vendor {
            missing_profiles.push(VirtualHidProfile::VendorDongle);
        }
        missing_profiles.extend([
            VirtualHidProfile::Interface3,
            VirtualHidProfile::Interface4,
            VirtualHidProfile::Interface5,
        ]);

        let synthetic_head = build_sdl_hid_info_list(&missing_profiles);
        let result = if head.is_null() {
            synthetic_head
        } else {
            if !tail.is_null() {
                (*tail).next = synthetic_head;
            }
            head
        };

        remember_augmented_sdl_enumeration(result);
        result
    }

    unsafe fn rewrite_sdl_hid_device_info(
        device_info: *mut SdlHidDeviceInfo,
        profile: VirtualHidProfile,
    ) {
        if device_info.is_null() {
            return;
        }

        (*device_info).path = leak_c_string(&virtual_device_path(profile));
        (*device_info).vendor_id = VIRTUAL_VENDOR_ID;
        (*device_info).product_id = VIRTUAL_PRODUCT_ID;
        (*device_info).serial_number = leak_wide_string(VIRTUAL_SERIAL);
        (*device_info).release_number = VIRTUAL_VERSION_NUMBER;
        (*device_info).manufacturer_string = leak_wide_string(VIRTUAL_MANUFACTURER);
        (*device_info).product_string = leak_wide_string(VIRTUAL_PRODUCT);
        (*device_info).usage_page = profile.caps().usage_page;
        (*device_info).usage = profile.caps().usage;
        (*device_info).interface_number = profile.interface_number() as c_int;
        (*device_info).interface_class = 0;
        (*device_info).interface_subclass = 0;
        (*device_info).interface_protocol = 0;
        (*device_info).bus_type = 1;
        debug_line(&format!(
            "[crosspuck] SDL_hid_enumerate rewrite profile={} path={:?}",
            profile.label(),
            virtual_device_path(profile)
        ));
    }

    unsafe fn build_sdl_hid_info_list(profiles: &[VirtualHidProfile]) -> *mut SdlHidDeviceInfo {
        let mut head: *mut SdlHidDeviceInfo = ptr::null_mut();
        let mut tail: *mut SdlHidDeviceInfo = ptr::null_mut();
        for profile in profiles.iter().copied() {
            let node = Box::into_raw(Box::new(SdlHidDeviceInfo {
                path: leak_c_string(&virtual_device_path(profile)),
                vendor_id: VIRTUAL_VENDOR_ID,
                product_id: VIRTUAL_PRODUCT_ID,
                serial_number: leak_wide_string(VIRTUAL_SERIAL),
                release_number: VIRTUAL_VERSION_NUMBER,
                manufacturer_string: leak_wide_string(VIRTUAL_MANUFACTURER),
                product_string: leak_wide_string(VIRTUAL_PRODUCT),
                usage_page: profile.caps().usage_page,
                usage: profile.caps().usage,
                interface_number: profile.interface_number() as c_int,
                interface_class: 0,
                interface_subclass: 0,
                interface_protocol: 0,
                bus_type: 1,
                next: ptr::null_mut(),
            }));

            if head.is_null() {
                head = node;
            } else {
                (*tail).next = node;
            }
            tail = node;
            debug_line(&format!(
                "[crosspuck] SDL_hid_enumerate append synthetic profile={} path={:?}",
                profile.label(),
                virtual_device_path(profile)
            ));
        }
        head
    }

    fn leak_c_string(value: &str) -> *mut c_char {
        CString::new(value).unwrap_or_default().into_raw()
    }

    fn leak_wide_string(value: &str) -> *mut u16 {
        let mut units = value.encode_utf16().collect::<Vec<_>>();
        units.push(0);
        Box::leak(units.into_boxed_slice()).as_mut_ptr()
    }

    fn remember_augmented_sdl_enumeration(head: *mut SdlHidDeviceInfo) {
        if head.is_null() {
            return;
        }
        let heads = SDL_AUGMENTED_ENUMERATIONS.get_or_init(|| Mutex::new(Vec::new()));
        if let Ok(mut guard) = heads.lock() {
            let value = head as usize;
            if !guard.contains(&value) {
                guard.push(value);
            }
        }
    }

    fn is_augmented_sdl_enumeration(head: *mut SdlHidDeviceInfo) -> bool {
        let Some(heads) = SDL_AUGMENTED_ENUMERATIONS.get() else {
            return false;
        };
        let value = head as usize;
        heads.lock().is_ok_and(|guard| guard.contains(&value))
    }

    unsafe fn remember_sdl_feature_command(device: *mut c_void, data: *const u8, len: usize) {
        if device.is_null() || data.is_null() || len < 2 {
            return;
        }

        let Some(profile) = virtual_profile_for_fake_sdl_device(device) else {
            return;
        };
        let device_value = device as usize;
        let report_id = *data;
        let command = *data.add(1);
        let request_value = if len > 3 { *data.add(3) } else { 0 };
        let commands = SDL_FEATURE_COMMANDS.get_or_init(|| Mutex::new(Vec::new()));
        if let Ok(mut guard) = commands.lock() {
            if let Some(existing) = guard
                .iter_mut()
                .find(|tracked| tracked.device == device_value && tracked.report_id == report_id)
            {
                existing.command = command;
                existing.request_value = request_value;
            } else {
                guard.push(SdlFeatureCommand {
                    device: device_value,
                    report_id,
                    command,
                    request_value,
                });
            }
        }

        if let Some(response) =
            input_report_response_for_feature_command(profile, report_id, command, request_value)
        {
            queue_pending_input_report(device_value, response);
        }
    }

    fn last_sdl_feature_command(device: *mut c_void, report_id: u8) -> Option<SdlFeatureCommand> {
        let Some(commands) = SDL_FEATURE_COMMANDS.get() else {
            return None;
        };
        let device_value = device as usize;
        commands.lock().ok().and_then(|guard| {
            guard
                .iter()
                .rev()
                .find(|tracked| tracked.device == device_value && tracked.report_id == report_id)
                .cloned()
        })
    }

    unsafe fn synthesize_sdl_feature_report(
        api: &str,
        device: *mut c_void,
        data: *mut u8,
        len: usize,
    ) -> c_int {
        if data.is_null() || len == 0 {
            return -1;
        }

        let report_id = *data;
        let command = last_sdl_feature_command(device, report_id);
        ptr::write_bytes(data, 0, len);
        let profile = virtual_profile_for_fake_sdl_device(device);
        let response =
            profile.and_then(|profile| sdl_feature_response(profile, report_id, command.as_ref()));
        let response_len = response
            .as_ref()
            .map(|bytes| copy_sdl_feature_response(data, len, bytes))
            .unwrap_or_else(|| {
                *data = report_id;
                len
            });
        let command_text = command
            .as_ref()
            .map(|value| format!("0x{:02X}/0x{:02X}", value.command, value.request_value))
            .unwrap_or_else(|| "-".to_string());
        log_hidd_report_call(&format!(
            "[crosspuck] {api} virtual device={device:p} len={} report_id=0x{report_id:02X} command={} synthesized={} response_head={}",
            len,
            command_text,
            response.is_some(),
            hex_head(data as *const u8, response_len.min(16) as u32)
        ));
        response_len.min(c_int::MAX as usize) as c_int
    }

    fn sdl_feature_response(
        profile: VirtualHidProfile,
        report_id: u8,
        command: Option<&SdlFeatureCommand>,
    ) -> Option<Vec<u8>> {
        if !profile.is_active_controller_slot() {
            return match (profile, report_id, command.map(|value| value.command)) {
                (VirtualHidProfile::VendorDongle, 0x02, Some(0xB4)) => {
                    Some(dongle_wireless_state_response())
                }
                _ => None,
            };
        }

        match (report_id, command.map(|value| value.command)) {
            (0x02, Some(0xA3)) => Some(triton_controller_info_response(0x02)),
            (0x02, Some(0xB4)) => Some(dongle_wireless_state_response()),
            (0x02, Some(0x83)) => Some(triton_attributes_response(0x02)),
            (0x02, Some(0xAE)) => Some(triton_string_attribute_response(
                0x02,
                command.map(|value| value.request_value).unwrap_or(1),
            )),
            (0x01, Some(0x83)) => Some(triton_attributes_response(0x01)),
            (0x01, Some(0xAE)) => Some(triton_string_attribute_response(
                0x01,
                command.map(|value| value.request_value).unwrap_or(1),
            )),
            (0x01, Some(0xA3)) => Some(triton_controller_info_response(0x01)),
            _ => None,
        }
    }

    fn input_report_response_for_feature_command(
        profile: VirtualHidProfile,
        report_id: u8,
        command: u8,
        request_value: u8,
    ) -> Option<Vec<u8>> {
        if !profile.is_active_controller_slot() {
            return None;
        }

        match (report_id, command) {
            (0x01, 0xAE) => Some(triton_string_attribute_input_response(request_value)),
            _ => None,
        }
    }

    fn queue_pending_input_report(device: usize, report: Vec<u8>) {
        let head = unsafe { hex_head(report.as_ptr(), report.len().min(16) as u32) };
        let queue = PENDING_INPUT_REPORTS.get_or_init(|| Mutex::new(VecDeque::new()));
        if let Ok(mut guard) = queue.lock() {
            guard.push_back(PendingInputReport { device, report });
            debug_line(&format!(
                "[crosspuck] queued pending input report device=0x{device:X} count={} head={head}",
                guard
                    .iter()
                    .filter(|pending| pending.device == device)
                    .count()
            ));
        }
    }

    fn dongle_wireless_state_response() -> Vec<u8> {
        let mut response = vec![0x02, 0xB4, 0x01, 0x02];
        response.resize(64, 0);
        response
    }

    fn triton_controller_info_response(report_id: u8) -> Vec<u8> {
        let mut response = vec![
            report_id, 0xA3, 0x18, 0x26, 0xAB, 0x01, 0x71, 0x80, 0xC0, 0x7A, 0x09, b'F', b'X',
            b'A', b'9', b'9', b'6', b'1', b'3', b'0', b'2', b'5', b'0', b'B',
        ];
        response.resize(64, 0);
        response
    }

    fn triton_string_attribute_response(report_id: u8, attribute_tag: u8) -> Vec<u8> {
        const ATTRIB_STR_BOARD_SERIAL: u8 = 0;
        const ATTRIB_STR_UNIT_SERIAL: u8 = 1;
        const TRITON_SERIAL: &[u8] = b"FXA996130250B";

        let attribute_value = match attribute_tag {
            ATTRIB_STR_BOARD_SERIAL | ATTRIB_STR_UNIT_SERIAL => TRITON_SERIAL,
            _ => TRITON_SERIAL,
        };
        let mut response = Vec::with_capacity(24);
        response.push(report_id);
        response.push(0xAE);
        response.push(21);
        response.push(attribute_tag);
        let mut value = [0_u8; 20];
        let count = value.len().min(attribute_value.len());
        value[..count].copy_from_slice(&attribute_value[..count]);
        response.extend_from_slice(&value);
        response
    }

    fn triton_string_attribute_input_response(attribute_tag: u8) -> Vec<u8> {
        const ATTRIB_STR_BOARD_SERIAL: u8 = 0;
        const ATTRIB_STR_UNIT_SERIAL: u8 = 1;
        const TRITON_SERIAL: &[u8] = b"FXA996130250B";

        let attribute_value = match attribute_tag {
            ATTRIB_STR_BOARD_SERIAL | ATTRIB_STR_UNIT_SERIAL => TRITON_SERIAL,
            _ => TRITON_SERIAL,
        };

        let mut response = Vec::with_capacity(54);
        response.push(0xAE);
        response.push(21);
        response.push(attribute_tag);
        let mut value = [0_u8; 20];
        let count = value.len().min(attribute_value.len());
        value[..count].copy_from_slice(&attribute_value[..count]);
        response.extend_from_slice(&value);
        response.resize(54, 0);
        response
    }

    fn triton_attributes_response(report_id: u8) -> Vec<u8> {
        const ATTRIB_UNIQUE_ID: u8 = 0;
        const ATTRIB_PRODUCT_ID: u8 = 1;
        const ATTRIB_CAPABILITIES: u8 = 2;
        const ATTRIB_FIRMWARE_VERSION: u8 = 3;
        const ATTRIB_FIRMWARE_BUILD_TIME: u8 = 4;
        const ATTRIB_RADIO_FIRMWARE_BUILD_TIME: u8 = 5;
        const ATTRIB_RADIO_DEVICE_ID0: u8 = 6;
        const ATTRIB_RADIO_DEVICE_ID1: u8 = 7;
        const ATTRIB_DONGLE_FIRMWARE_BUILD_TIME: u8 = 8;
        const ATTRIB_BOARD_REVISION: u8 = 9;
        const ATTRIB_BOOTLOADER_BUILD_TIME: u8 = 10;
        const ATTRIB_CONNECTION_INTERVAL_IN_US: u8 = 11;

        let attributes: [(u8, u32); 12] = [
            (ATTRIB_UNIQUE_ID, 0x7101_AB26),
            (ATTRIB_PRODUCT_ID, 0x0000_1302),
            (ATTRIB_CAPABILITIES, 0x4160_BFFF),
            (ATTRIB_FIRMWARE_VERSION, 0x6A10_91CE),
            (ATTRIB_FIRMWARE_BUILD_TIME, 0x6A10_91CE),
            (ATTRIB_RADIO_FIRMWARE_BUILD_TIME, 0x0000_0000),
            (ATTRIB_RADIO_DEVICE_ID0, 0x7101_AB26),
            (ATTRIB_RADIO_DEVICE_ID1, 0x097A_C080),
            (ATTRIB_DONGLE_FIRMWARE_BUILD_TIME, 0x6A10_91CF),
            (ATTRIB_BOARD_REVISION, 48),
            (ATTRIB_BOOTLOADER_BUILD_TIME, 0x0000_0000),
            (ATTRIB_CONNECTION_INTERVAL_IN_US, 4_000),
        ];

        let mut response = Vec::with_capacity(64);
        response.push(report_id);
        response.push(0x83);
        response.push((attributes.len() * 5) as u8);
        for (tag, value) in attributes {
            response.push(tag);
            response.extend_from_slice(&value.to_le_bytes());
        }
        response.resize(64, 0);
        response
    }

    unsafe fn copy_sdl_feature_response(data: *mut u8, len: usize, response: &[u8]) -> usize {
        let count = len.min(response.len());
        ptr::copy_nonoverlapping(response.as_ptr(), data, count);
        count
    }

    unsafe fn maybe_disconnect_on_controller_log_write(
        handle: HANDLE,
        buffer: *const c_void,
        bytes_to_write: u32,
    ) {
        let enabled = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_controller_workitem_exit);
        if !enabled
            || !virtual_stream_connected()
            || !active_virtual_session()
            || !is_controller_log_handle(handle)
            || buffer.is_null()
            || bytes_to_write == 0
        {
            return;
        }

        let inspect_len = (bytes_to_write as usize).min(4096);
        let bytes = slice::from_raw_parts(buffer as *const u8, inspect_len);
        let text = String::from_utf8_lossy(bytes);
        if !text.contains("Exiting workitem thread") {
            return;
        }

        debug_line(&format!(
            "[crosspuck] controller log signaled workitem exit; disconnecting virtual HID stream handle={} bytes_to_write={}",
            handle_label(handle),
            bytes_to_write
        ));
        disconnect_virtual_stream("controller workitem exit");
    }

    fn maybe_disconnect_on_steam_assert_path(api: &str, path: &str) {
        let enabled = CONFIG
            .get()
            .is_some_and(|config| config.disconnect_on_steam_assert_dump);
        if !enabled {
            return;
        }

        if !looks_like_steam_assert_dump_path(path) {
            return;
        }

        let connected = virtual_stream_connected();
        let active = active_virtual_session();
        let should_disconnect = connected && active;
        debug_line(&format!(
            "[crosspuck] {api} detected Steam assert dump path connected={} active_virtual_session={} should_disconnect={} path={path:?}",
            connected, active, should_disconnect
        ));
        if !should_disconnect {
            return;
        }

        disconnect_virtual_stream("Steam assert dump creation");
    }

    fn looks_like_steam_assert_dump_path(path: &str) -> bool {
        let lower = path.replace('/', "\\").to_ascii_lowercase();
        lower.contains("\\dumps\\assert_steam.exe_")
    }

    fn looks_like_controller_log_path(path: &str) -> bool {
        let lower = path.replace('/', "\\").to_ascii_lowercase();
        lower.ends_with("\\logs\\controller.txt")
    }

    fn looks_interesting_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        lower.contains("hid")
            || lower.contains("vid_")
            || lower.contains("pid_")
            || lower.contains("mi_")
            || lower.contains("controller")
            || lower.contains("gamepad")
    }

    fn hid_interface_guid() -> GUID {
        GUID {
            data1: 0x4D1E55B2,
            data2: 0xF16F,
            data3: 0x11CF,
            data4: [0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30],
        }
    }

    fn guid_eq(left: &GUID, right: &GUID) -> bool {
        left.data1 == right.data1
            && left.data2 == right.data2
            && left.data3 == right.data3
            && left.data4 == right.data4
    }

    fn is_hid_interface_guid(guid: *const GUID) -> bool {
        let Some(guid) = (unsafe { guid.as_ref() }) else {
            return false;
        };
        guid_eq(guid, &hid_interface_guid())
    }

    fn synthetic_profile_for_failed_member_index(
        device_info_set: HANDLE,
        member_index: u32,
    ) -> Option<VirtualHidProfile> {
        let base = synthetic_enum_base_index(device_info_set, member_index);
        let profiles: &[VirtualHidProfile] = if base == 0 {
            &[
                VirtualHidProfile::Main,
                VirtualHidProfile::VendorDongle,
                VirtualHidProfile::Interface3,
                VirtualHidProfile::Interface4,
                VirtualHidProfile::Interface5,
            ]
        } else if base == 1 {
            &[
                VirtualHidProfile::VendorDongle,
                VirtualHidProfile::Interface3,
                VirtualHidProfile::Interface4,
                VirtualHidProfile::Interface5,
            ]
        } else {
            &[
                VirtualHidProfile::Interface3,
                VirtualHidProfile::Interface4,
                VirtualHidProfile::Interface5,
            ]
        };
        let offset = member_index.checked_sub(base)? as usize;
        profiles.get(offset).copied()
    }

    fn synthetic_enum_base_index(device_info_set: HANDLE, member_index: u32) -> u32 {
        let set_value = device_info_set as usize;
        let mut guard = match SYNTHETIC_ENUM_BASES
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
        {
            Ok(guard) => guard,
            Err(_) => return member_index,
        };

        if let Some((_, base)) = guard
            .iter()
            .find(|(stored_set, _)| *stored_set == set_value)
        {
            return *base;
        }

        guard.push((set_value, member_index));
        member_index
    }

    fn virtual_profile_for_interface_reserved(reserved: usize) -> Option<VirtualHidProfile> {
        if reserved & 0xFFFF_FFFF_FFFF_0000 != VIRTUAL_INTERFACE_RESERVED_BASE {
            return None;
        }

        match (reserved & 0xFFFF) as u8 {
            2 => Some(VirtualHidProfile::Main),
            3 => Some(VirtualHidProfile::Interface3),
            4 => Some(VirtualHidProfile::Interface4),
            5 => Some(VirtualHidProfile::Interface5),
            6 => Some(VirtualHidProfile::VendorDongle),
            _ => None,
        }
    }

    unsafe fn virtual_profile_for_device_interface_data(
        device_interface_data: *mut c_void,
    ) -> Option<VirtualHidProfile> {
        if device_interface_data.is_null() {
            return None;
        }

        let data = &*(device_interface_data as *const SpDeviceInterfaceData);
        virtual_profile_for_interface_reserved(data.reserved)
    }

    unsafe fn write_synthetic_device_interface_data(
        device_interface_data: *mut c_void,
        interface_class_guid: *const GUID,
        profile: VirtualHidProfile,
    ) -> bool {
        if device_interface_data.is_null() {
            return false;
        }

        let data = &mut *(device_interface_data as *mut SpDeviceInterfaceData);
        if data.cb_size == 0 {
            data.cb_size = std::mem::size_of::<SpDeviceInterfaceData>() as u32;
        }
        data.interface_class_guid = interface_class_guid
            .as_ref()
            .copied()
            .unwrap_or_else(hid_interface_guid);
        data.flags = SPINT_ACTIVE;
        data.reserved = profile.interface_reserved();
        true
    }

    unsafe fn proc_name_label(proc_name: PCSTR) -> String {
        if proc_name.is_null() {
            return "null".to_string();
        }

        let value = proc_name as usize;
        if value >> 16 == 0 {
            format!("#{}", value & 0xFFFF)
        } else {
            narrow_z_to_string(proc_name).unwrap_or_else(|| "<invalid>".to_string())
        }
    }

    unsafe fn hex_head(buffer: *const u8, len: u32) -> String {
        hex_bytes(buffer, len, 16)
    }

    unsafe fn hex_bytes(buffer: *const u8, len: u32, max_bytes: usize) -> String {
        if buffer.is_null() || len == 0 {
            return "-".to_string();
        }

        let len = len as usize;
        let count = len.min(max_bytes);
        let mut rendered = slice::from_raw_parts(buffer, count)
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        if len > count {
            rendered.push_str(&format!(" ...(+{} bytes)", len - count));
        }
        rendered
    }

    unsafe fn synthesize_device_interface_detail_w(
        profile: VirtualHidProfile,
        device_interface_detail_data: *mut c_void,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> BOOL {
        let path = virtual_device_path(profile);
        let required = 4 + ((path.encode_utf16().count() + 1) * 2) as u32;
        if !required_size.is_null() {
            *required_size = required;
        }
        write_synthetic_device_info_data(device_info_data, profile, "SyntheticDetailW");

        if device_interface_detail_data.is_null() || device_interface_detail_data_size < required {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInterfaceDetailW synthetic needs larger buffer profile={} required={} size={} path={path:?}",
                profile.label(),
                required,
                device_interface_detail_data_size
            ));
            return FALSE;
        }

        if write_detail_w_path(
            device_interface_detail_data,
            device_interface_detail_data_size,
            &path,
        ) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInterfaceDetailW synthetic profile={} size={} required={} path={path:?}",
                profile.label(),
                device_interface_detail_data_size,
                required
            ));
            TRUE
        } else {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            FALSE
        }
    }

    unsafe fn synthesize_device_interface_detail_a(
        profile: VirtualHidProfile,
        device_interface_detail_data: *mut c_void,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> BOOL {
        let path = virtual_device_path(profile);
        let required = 4 + path.len() as u32 + 1;
        if !required_size.is_null() {
            *required_size = required;
        }
        write_synthetic_device_info_data(device_info_data, profile, "SyntheticDetailA");

        if device_interface_detail_data.is_null() || device_interface_detail_data_size < required {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInterfaceDetailA synthetic needs larger buffer profile={} required={} size={} path={path:?}",
                profile.label(),
                required,
                device_interface_detail_data_size
            ));
            return FALSE;
        }

        if write_detail_a_path(
            device_interface_detail_data,
            device_interface_detail_data_size,
            &path,
        ) {
            debug_line(&format!(
                "[crosspuck] SetupDiGetDeviceInterfaceDetailA synthetic profile={} size={} required={} path={path:?}",
                profile.label(),
                device_interface_detail_data_size,
                required
            ));
            TRUE
        } else {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            FALSE
        }
    }

    unsafe fn write_synthetic_device_info_data(
        device_info_data: *mut c_void,
        profile: VirtualHidProfile,
        source: &str,
    ) {
        if device_info_data.is_null() {
            return;
        }

        let data = &mut *(device_info_data as *mut SpDevinfoData);
        if data.cb_size == 0 {
            data.cb_size = std::mem::size_of::<SpDevinfoData>() as u32;
        }
        data.class_guid = hid_interface_guid();
        data.dev_inst = profile.synthetic_devinst();
        data.reserved = 0;
        remember_virtual_devinst(data.dev_inst, profile, source);
    }

    unsafe fn detail_w_path(detail: *mut c_void) -> Option<String> {
        if detail.is_null() {
            return None;
        }

        let path_ptr = (detail as *const u8).add(4) as PCWSTR;
        wide_z_to_string(path_ptr)
    }

    unsafe fn detail_a_path(detail: *mut c_void) -> Option<String> {
        if detail.is_null() {
            return None;
        }

        let path_ptr = (detail as *const u8).add(4) as PCSTR;
        narrow_z_to_string(path_ptr)
    }

    fn rewrite_hid_device_path(path: &str) -> Option<String> {
        let lower = path.to_ascii_lowercase();
        let profile = if lower.contains("vid_845e&pid_0001") {
            VirtualHidProfile::Main
        } else if lower.contains("vid_845e&pid_0002") {
            VirtualHidProfile::VendorDongle
        } else {
            return None;
        };

        let hid_start = lower.find("hid#")?;
        let class_guid_start = lower.rfind("#{")?;
        let prefix = &path[..hid_start + "hid#".len()];
        let suffix = &path[class_guid_start..];
        let instance = virtual_path_instance(profile);

        Some(format!("{prefix}vid_28de&pid_1304&{instance}{suffix}"))
    }

    fn virtual_device_path(profile: VirtualHidProfile) -> String {
        format!(
            r"\\?\hid#vid_28de&pid_1304&{}#{}",
            virtual_path_instance(profile),
            HID_INTERFACE_GUID_STRING
        )
    }

    fn virtual_path_instance(profile: VirtualHidProfile) -> String {
        format!(
            "mi_{:02x}&col{:02x}#{}&0&{:04x}",
            profile.interface_number(),
            profile.collection_number(),
            VIRTUAL_SERIAL.to_ascii_lowercase(),
            profile.instance_suffix()
        )
    }

    unsafe fn write_detail_w_path(detail: *mut c_void, detail_size: u32, value: &str) -> bool {
        if detail.is_null() || detail_size <= 4 {
            return false;
        }

        let path_ptr = (detail as *mut u8).add(4) as *mut u16;
        let capacity = (detail_size as usize - 4) / 2;
        let encoded: Vec<u16> = value.encode_utf16().collect();
        if encoded.len() + 1 > capacity {
            return false;
        }

        ptr::copy_nonoverlapping(encoded.as_ptr(), path_ptr, encoded.len());
        *path_ptr.add(encoded.len()) = 0;
        true
    }

    unsafe fn write_detail_a_path(detail: *mut c_void, detail_size: u32, value: &str) -> bool {
        if detail.is_null() || detail_size <= 4 {
            return false;
        }

        let path_ptr = (detail as *mut u8).add(4);
        let capacity = detail_size as usize - 4;
        let bytes = value.as_bytes();
        if bytes.len() + 1 > capacity {
            return false;
        }

        ptr::copy_nonoverlapping(bytes.as_ptr(), path_ptr, bytes.len());
        *path_ptr.add(bytes.len()) = 0;
        true
    }

    fn guid_label(guid: *const GUID) -> String {
        if guid.is_null() {
            return "null".to_string();
        }

        let guid = unsafe { *guid };
        guid_value_label(&guid)
    }

    fn guid_value_label(guid: &GUID) -> String {
        format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            guid.data1,
            guid.data2,
            guid.data3,
            guid.data4[0],
            guid.data4[1],
            guid.data4[2],
            guid.data4[3],
            guid.data4[4],
            guid.data4[5],
            guid.data4[6],
            guid.data4[7]
        )
    }

    fn bool_label(value: BOOL) -> &'static str {
        if value == TRUE {
            "TRUE"
        } else {
            "FALSE"
        }
    }

    fn handle_label(handle: HANDLE) -> String {
        if let Some(profile) = virtual_profile_for_handle(handle) {
            format!("virtual:{}", profile.label())
        } else {
            format!("{handle:p}")
        }
    }

    fn load_library(name: &str) -> usize {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { LoadLibraryW(wide.as_ptr()) as usize }
    }

    unsafe fn resolve_real_hid_proc(name: &str) -> Option<*const c_void> {
        let module = *REAL_HID_MODULE.get_or_init(load_real_hid_module);
        if module == 0 {
            return None;
        }

        let mut proc_name = Vec::with_capacity(name.len() + 1);
        proc_name.extend_from_slice(name.as_bytes());
        proc_name.push(0);
        GetProcAddress(module as _, proc_name.as_ptr()).map(|proc| proc as *const c_void)
    }

    fn load_real_hid_module() -> usize {
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let path = system_root.join("System32").join("hid.dll");
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe { LoadLibraryW(wide.as_ptr()) as usize }
    }

    unsafe fn call_real_hidd_get_attributes(device: HANDLE, attributes: *mut HiddAttributes) -> u8 {
        type RealFn = unsafe extern "system" fn(HANDLE, *mut HiddAttributes) -> u8;
        resolve_real_hid_proc("HidD_GetAttributes")
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, attributes))
            .unwrap_or(FALSE_U8)
    }

    unsafe fn call_real_hidd_get_preparsed_data(
        device: HANDLE,
        preparsed_data: *mut *mut c_void,
    ) -> u8 {
        type RealFn = unsafe extern "system" fn(HANDLE, *mut *mut c_void) -> u8;
        resolve_real_hid_proc("HidD_GetPreparsedData")
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, preparsed_data))
            .unwrap_or(FALSE_U8)
    }

    unsafe fn call_real_hidd_free_preparsed_data(preparsed_data: *mut c_void) -> u8 {
        type RealFn = unsafe extern "system" fn(*mut c_void) -> u8;
        resolve_real_hid_proc("HidD_FreePreparsedData")
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(preparsed_data))
            .unwrap_or(FALSE_U8)
    }

    unsafe fn call_real_hidd_string(
        name: &str,
        device: HANDLE,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u8 {
        type RealFn = unsafe extern "system" fn(HANDLE, *mut c_void, u32) -> u8;
        resolve_real_hid_proc(name)
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device, buffer, buffer_len))
            .unwrap_or(FALSE_U8)
    }

    unsafe fn call_real_hidd_indexed_string(
        device: HANDLE,
        string_index: u32,
        buffer: *mut c_void,
        buffer_len: u32,
    ) -> u8 {
        type RealFn = unsafe extern "system" fn(HANDLE, u32, *mut c_void, u32) -> u8;
        resolve_real_hid_proc("HidD_GetIndexedString")
            .map(|ptr| {
                std::mem::transmute::<_, RealFn>(ptr)(device, string_index, buffer, buffer_len)
            })
            .unwrap_or(FALSE_U8)
    }

    unsafe fn call_real_hidd_report(
        name: &str,
        device: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_len: u32,
    ) -> u8 {
        type RealFn = unsafe extern "system" fn(HANDLE, *mut c_void, u32) -> u8;
        resolve_real_hid_proc(name)
            .map(|ptr| {
                std::mem::transmute::<_, RealFn>(ptr)(device, report_buffer, report_buffer_len)
            })
            .unwrap_or(FALSE_U8)
    }

    unsafe fn call_real_hidd_handle_bool(name: &str, device: HANDLE) -> u8 {
        type RealFn = unsafe extern "system" fn(HANDLE) -> u8;
        resolve_real_hid_proc(name)
            .map(|ptr| std::mem::transmute::<_, RealFn>(ptr)(device))
            .unwrap_or(FALSE_U8)
    }
}

#[cfg(not(windows))]
pub fn platform_note() -> &'static str {
    "crosspuck-hid-proxy builds the replay core on non-Windows targets; the hid.dll proxy layer is compiled on Windows only."
}
