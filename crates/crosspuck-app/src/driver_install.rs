use crate::bundle::{self, BundleError, EmbeddedDriver};
use std::cmp::Ordering;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const BOTTLE_PATH_ENV: &str = "CROSSPUCK_BOTTLE_PATH";
const EMBEDDED_DRIVER_PATH_ENV: &str = "CROSSPUCK_EMBEDDED_DRIVER_PATH";
const DEFAULT_STEAM_BOTTLE_NAME: &str = "Steam";
const DRIVER_TARGET_RELATIVE_PATH: &[&str] =
    &["target", "x86_64-pc-windows-gnu", "release", "hid.dll"];
const CROSSOVER_BOTTLES_RELATIVE_PATH: &[&str] =
    &["Library", "Application Support", "CrossOver", "Bottles"];
const HID_DLL_OVERRIDE_VALUE: &str = "native,builtin";
const WINE_HID_OVERRIDE_REG_FILE_NAME: &str = "crosspuck-wine-override.reg";
const USER_REG_FILE_NAME: &str = "user.reg";
const USER_REG_DLL_OVERRIDES_SECTION: &str = r"[Software\\Wine\\DllOverrides]";
const USER_REG_HID_OVERRIDE_NAME: &str = "hid";
const CX_BOTTLE_CONF_FILE_NAME: &str = "cxbottle.conf";
const CROSSOVER_APP_PATH: &str = "/Applications/CrossOver.app";
const CROSSOVER_PREVIEW_APP_PATH: &str = "/Applications/CrossOver Preview.app";
const CROSSOVER_HOSTED_WINE_RELATIVE_PATH: &[&str] = &[
    "Contents",
    "SharedSupport",
    "CrossOver",
    "CrossOver-Hosted Application",
    "wine",
];
const CROSSOVER_BIN_WINE_RELATIVE_PATH: &[&str] =
    &["Contents", "SharedSupport", "CrossOver", "bin", "wine"];

#[derive(Clone, Debug)]
pub struct DriverInstallContext {
    pub resources_dir: Option<PathBuf>,
    pub embedded_driver_path: Option<PathBuf>,
    pub repo_root: Option<PathBuf>,
    pub bottle_path: Option<PathBuf>,
    pub crossover_bottles_dir: Option<PathBuf>,
    pub crossover_app_paths: Vec<PathBuf>,
    pub manage_wine_registry: bool,
    pub allow_development_fallbacks: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverInstallState {
    BundledDriverMissing,
    BottleNotFound,
    SteamExeNotFound,
    NotInstalled,
    UpdateAvailable,
    Installed,
    CheckFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverInstallStatus {
    pub state: DriverInstallState,
    pub status_title: String,
    pub action_title: String,
    pub action_enabled: bool,
    pub embedded_driver: Option<EmbeddedDriverSummary>,
    pub bottle_path: Option<PathBuf>,
    pub steam_dir: Option<PathBuf>,
    pub target_dll: Option<PathBuf>,
    pub installed_sha256: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedDriverSummary {
    pub dll_path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverInstallResult {
    pub target_dll: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub installed_sha256: String,
    pub registry_targets: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverUninstallResult {
    pub target_dll: PathBuf,
    pub removed_driver: bool,
    pub registry_targets: Vec<String>,
}

#[derive(Debug)]
pub enum DriverInstallError {
    Bundle(BundleError),
    BottleNotFound,
    SteamExeNotFound {
        bottle_path: PathBuf,
    },
    System32TargetRefused(PathBuf),
    VerificationFailed {
        expected: String,
        actual: String,
    },
    CrossoverWineNotFound {
        bottle_path: PathBuf,
    },
    RegistryImportFailed {
        wine_path: PathBuf,
        status: Option<i32>,
        stdout: String,
        stderr: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl DriverInstallContext {
    pub fn from_environment(resources_dir: Option<PathBuf>) -> Self {
        Self {
            resources_dir,
            embedded_driver_path: env::var_os(EMBEDDED_DRIVER_PATH_ENV).map(PathBuf::from),
            repo_root: Some(default_repo_root()),
            bottle_path: env::var_os(BOTTLE_PATH_ENV).map(PathBuf::from),
            crossover_bottles_dir: default_crossover_bottles_dir(),
            crossover_app_paths: default_crossover_app_paths(),
            manage_wine_registry: true,
            allow_development_fallbacks: cfg!(debug_assertions),
        }
    }
}

impl Default for DriverInstallContext {
    fn default() -> Self {
        Self {
            resources_dir: None,
            embedded_driver_path: None,
            repo_root: None,
            bottle_path: None,
            crossover_bottles_dir: None,
            crossover_app_paths: Vec::new(),
            manage_wine_registry: true,
            allow_development_fallbacks: false,
        }
    }
}

pub fn check_driver_install_status(context: &DriverInstallContext) -> DriverInstallStatus {
    match load_embedded_driver(context) {
        Ok(embedded_driver) => {
            match check_driver_install_status_with_embedded(context, embedded_driver) {
                Ok(status) => status,
                Err(error) => check_failed_status(Some(error)),
            }
        }
        Err(error) => DriverInstallStatus::new(
            DriverInstallState::BundledDriverMissing,
            "Driver: Bundled driver missing.",
            "Install Steam Driver...",
            false,
        )
        .with_error(error.to_string()),
    }
}

pub fn install_driver(
    context: &DriverInstallContext,
) -> Result<DriverInstallResult, DriverInstallError> {
    install_driver_with_timestamp(context, &default_backup_timestamp())
}

pub fn install_driver_with_timestamp(
    context: &DriverInstallContext,
    backup_timestamp: &str,
) -> Result<DriverInstallResult, DriverInstallError> {
    let embedded_driver = load_embedded_driver(context).map_err(DriverInstallError::Bundle)?;
    let bottle_path = discover_bottle(context).ok_or(DriverInstallError::BottleNotFound)?;
    let steam_dir =
        find_steam_dir(&bottle_path).ok_or_else(|| DriverInstallError::SteamExeNotFound {
            bottle_path: bottle_path.clone(),
        })?;
    let target_dll = install_target_for_steam_dir(&steam_dir)?;

    if target_dll.is_file() {
        let installed_sha256 =
            bundle::sha256_file(&target_dll).map_err(DriverInstallError::Bundle)?;
        if installed_sha256 == embedded_driver.actual_sha256 {
            let registry_targets = apply_hid_registry_override(context, &bottle_path)?;
            return Ok(DriverInstallResult {
                target_dll,
                backup_path: None,
                installed_sha256,
                registry_targets,
            });
        }
    }

    let backup_path = if target_dll.is_file() {
        let backup_dir = steam_dir.join("crosspuck-backups");
        create_dir_all(&backup_dir)?;
        let backup_path = backup_dir.join(format!("hid.dll.{backup_timestamp}"));
        copy_file(&target_dll, &backup_path)?;
        Some(backup_path)
    } else {
        None
    };

    atomic_copy(&embedded_driver.dll_path, &target_dll)?;
    let installed_sha256 = bundle::sha256_file(&target_dll).map_err(DriverInstallError::Bundle)?;
    if installed_sha256 != embedded_driver.actual_sha256 {
        return Err(DriverInstallError::VerificationFailed {
            expected: embedded_driver.actual_sha256,
            actual: installed_sha256,
        });
    }

    let registry_targets = apply_hid_registry_override(context, &bottle_path)?;

    Ok(DriverInstallResult {
        target_dll,
        backup_path,
        installed_sha256,
        registry_targets,
    })
}

pub fn uninstall_driver(
    context: &DriverInstallContext,
) -> Result<DriverUninstallResult, DriverInstallError> {
    let bottle_path = discover_bottle(context).ok_or(DriverInstallError::BottleNotFound)?;
    let steam_dir =
        find_steam_dir(&bottle_path).ok_or_else(|| DriverInstallError::SteamExeNotFound {
            bottle_path: bottle_path.clone(),
        })?;
    let target_dll = install_target_for_steam_dir(&steam_dir)?;
    let removed_driver = remove_file_if_exists(&target_dll)?;

    Ok(DriverUninstallResult {
        target_dll,
        removed_driver,
        registry_targets: Vec::new(),
    })
}

pub fn load_embedded_driver(context: &DriverInstallContext) -> Result<EmbeddedDriver, BundleError> {
    let mut first_error = None;

    if let Some(resources_dir) = context.resources_dir.as_ref() {
        match EmbeddedDriver::from_resources_dir(resources_dir) {
            Ok(driver) => return Ok(driver),
            Err(error) => first_error = Some(error),
        }
    }

    if context.allow_development_fallbacks {
        if let Some(driver_path) = context.embedded_driver_path.as_ref() {
            if driver_path.is_file() {
                return EmbeddedDriver::from_standalone_dll(driver_path);
            }
        }

        if let Some(repo_root) = context.repo_root.as_ref() {
            let driver_path = DRIVER_TARGET_RELATIVE_PATH
                .iter()
                .fold(repo_root.clone(), |path, segment| path.join(segment));
            if driver_path.is_file() {
                return EmbeddedDriver::from_standalone_dll(driver_path);
            }
        }
    }

    Err(first_error.unwrap_or_else(|| {
        BundleError::MissingDriver(PathBuf::from("Contents/Resources/GuestDriver/hid.dll"))
    }))
}

pub fn discover_bottle(context: &DriverInstallContext) -> Option<PathBuf> {
    if let Some(bottle_path) = context.bottle_path.as_ref() {
        return bottle_path.is_dir().then(|| bottle_path.clone());
    }

    let bottles_dir = context.crossover_bottles_dir.as_ref()?;
    let steam_bottle = bottles_dir.join(DEFAULT_STEAM_BOTTLE_NAME);
    if steam_bottle.is_dir() {
        return Some(steam_bottle);
    }

    let mut candidates = read_dir_sorted(bottles_dir).ok()?;
    candidates.retain(|path| path.is_dir());
    candidates
        .into_iter()
        .find(|candidate| find_steam_dir(candidate).is_some())
}

pub fn find_steam_dir(bottle_path: impl AsRef<Path>) -> Option<PathBuf> {
    let drive_c = bottle_path.as_ref().join("drive_c");
    let mut stack = vec![drive_c];
    while let Some(path) = stack.pop() {
        let Ok(entries) = read_dir_sorted(&path) else {
            continue;
        };
        for entry in entries.into_iter().rev() {
            if entry.is_dir() {
                stack.push(entry);
                continue;
            }
            if is_steam_exe(&entry) {
                return entry.parent().map(Path::to_path_buf);
            }
        }
    }
    None
}

pub fn install_target_for_steam_dir(
    steam_dir: impl AsRef<Path>,
) -> Result<PathBuf, DriverInstallError> {
    let target = steam_dir.as_ref().join(bundle::DRIVER_DLL_NAME);
    if is_system32_hid_target(&target) {
        return Err(DriverInstallError::System32TargetRefused(target));
    }
    Ok(target)
}

fn check_driver_install_status_with_embedded(
    context: &DriverInstallContext,
    embedded_driver: EmbeddedDriver,
) -> Result<DriverInstallStatus, DriverInstallError> {
    let embedded_summary = Some(EmbeddedDriverSummary::from(&embedded_driver));
    let Some(bottle_path) = discover_bottle(context) else {
        return Ok(DriverInstallStatus::new(
            DriverInstallState::BottleNotFound,
            "Driver: Steam bottle not found.",
            "Install Steam Driver...",
            false,
        )
        .with_embedded(embedded_summary));
    };

    let Some(steam_dir) = find_steam_dir(&bottle_path) else {
        return Ok(DriverInstallStatus::new(
            DriverInstallState::SteamExeNotFound,
            "Driver: Steam.exe not found.",
            "Install Steam Driver...",
            false,
        )
        .with_embedded(embedded_summary)
        .with_bottle(bottle_path));
    };

    let target_dll = install_target_for_steam_dir(&steam_dir)?;
    if !target_dll.exists() {
        return Ok(DriverInstallStatus::new(
            DriverInstallState::NotInstalled,
            "Driver: Not installed.",
            "Install Steam Driver...",
            true,
        )
        .with_embedded(embedded_summary)
        .with_bottle(bottle_path)
        .with_steam_dir(steam_dir)
        .with_target(target_dll));
    }

    let installed_sha256 = bundle::sha256_file(&target_dll).map_err(DriverInstallError::Bundle)?;
    let registry_override_installed = is_hid_registry_override_installed(context, &bottle_path)?;
    let (state, status_title, action_title, action_enabled) =
        if installed_sha256 == embedded_driver.actual_sha256 && registry_override_installed {
            (
                DriverInstallState::Installed,
                "Driver: Already installed.",
                "Repair Steam Driver...",
                true,
            )
        } else if installed_sha256 == embedded_driver.actual_sha256 {
            (
                DriverInstallState::UpdateAvailable,
                "Driver: Wine override missing.",
                "Repair Steam Driver...",
                true,
            )
        } else {
            (
                DriverInstallState::UpdateAvailable,
                "Driver: Update available.",
                "Update Steam Driver...",
                true,
            )
        };

    Ok(
        DriverInstallStatus::new(state, status_title, action_title, action_enabled)
            .with_embedded(embedded_summary)
            .with_bottle(bottle_path)
            .with_steam_dir(steam_dir)
            .with_target(target_dll)
            .with_installed_sha256(installed_sha256),
    )
}

fn check_failed_status(error: Option<DriverInstallError>) -> DriverInstallStatus {
    let message = error.as_ref().map(ToString::to_string);
    let title = message
        .as_deref()
        .map(|message| format!("Driver: Check failed: {message}"))
        .unwrap_or_else(|| "Driver: Check failed.".to_string());
    DriverInstallStatus::new(
        DriverInstallState::CheckFailed,
        title,
        "Install Steam Driver...",
        false,
    )
    .with_error(message.unwrap_or_else(|| "unknown error".to_string()))
}

impl DriverInstallStatus {
    fn new(
        state: DriverInstallState,
        status_title: impl Into<String>,
        action_title: impl Into<String>,
        action_enabled: bool,
    ) -> Self {
        Self {
            state,
            status_title: status_title.into(),
            action_title: action_title.into(),
            action_enabled,
            embedded_driver: None,
            bottle_path: None,
            steam_dir: None,
            target_dll: None,
            installed_sha256: None,
            error: None,
        }
    }

    fn with_embedded(mut self, embedded_driver: Option<EmbeddedDriverSummary>) -> Self {
        self.embedded_driver = embedded_driver;
        self
    }

    fn with_bottle(mut self, bottle_path: PathBuf) -> Self {
        self.bottle_path = Some(bottle_path);
        self
    }

    fn with_steam_dir(mut self, steam_dir: PathBuf) -> Self {
        self.steam_dir = Some(steam_dir);
        self
    }

    fn with_target(mut self, target_dll: PathBuf) -> Self {
        self.target_dll = Some(target_dll);
        self
    }

    fn with_installed_sha256(mut self, installed_sha256: String) -> Self {
        self.installed_sha256 = Some(installed_sha256);
        self
    }

    fn with_error(mut self, error: String) -> Self {
        self.error = Some(error);
        self
    }
}

impl From<&EmbeddedDriver> for EmbeddedDriverSummary {
    fn from(value: &EmbeddedDriver) -> Self {
        Self {
            dll_path: value.dll_path.clone(),
            sha256: value.actual_sha256.clone(),
            size: value.actual_size,
        }
    }
}

fn is_steam_exe(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("Steam.exe"))
}

fn is_system32_hid_target(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(normal_component)
        .collect::<Vec<_>>();
    components.ends_with(&[
        "drive_c".to_string(),
        "windows".to_string(),
        "system32".to_string(),
        "hid.dll".to_string(),
    ]) || components.ends_with(&[
        "windows".to_string(),
        "system32".to_string(),
        "hid.dll".to_string(),
    ])
}

fn normal_component(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(value) => value.to_str().map(|value| value.to_ascii_lowercase()),
        _ => None,
    }
}

fn read_dir_sorted(path: &Path) -> Result<Vec<PathBuf>, io::Error> {
    let mut entries = fs::read_dir(path)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    entries.sort_by(compare_paths);
    Ok(entries)
}

fn compare_paths(left: &PathBuf, right: &PathBuf) -> Ordering {
    left.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .cmp(
            &right
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_ascii_lowercase(),
        )
}

fn create_dir_all(path: &Path) -> Result<(), DriverInstallError> {
    fs::create_dir_all(path).map_err(|source| DriverInstallError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn copy_file(from: &Path, to: &Path) -> Result<(), DriverInstallError> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|source| DriverInstallError::Io {
            path: to.to_path_buf(),
            source,
        })
}

fn atomic_copy(from: &Path, to: &Path) -> Result<(), DriverInstallError> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    let temp_path = parent.join(".hid.dll.crosspuck.tmp");
    let _ = fs::remove_file(&temp_path);
    copy_file(from, &temp_path)?;
    if let Ok(file) = fs::File::open(&temp_path) {
        let _ = file.sync_all();
    }
    fs::rename(&temp_path, to).map_err(|source| DriverInstallError::Io {
        path: to.to_path_buf(),
        source,
    })
}

fn remove_file_if_exists(path: &Path) -> Result<bool, DriverInstallError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(DriverInstallError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn apply_hid_registry_override(
    context: &DriverInstallContext,
    bottle_path: &Path,
) -> Result<Vec<String>, DriverInstallError> {
    if !context.manage_wine_registry {
        return Ok(Vec::new());
    }

    let reg_file_path = write_hid_override_reg_file(bottle_path)?;
    let tool = find_crossover_wine_tool(context, bottle_path)?;
    import_registry_file(&tool, bottle_path, &reg_file_path)?;
    Ok(vec![format!(
        "{}: {}",
        tool.app_name(),
        reg_file_path.display()
    )])
}

fn is_hid_registry_override_installed(
    context: &DriverInstallContext,
    bottle_path: &Path,
) -> Result<bool, DriverInstallError> {
    if !context.manage_wine_registry {
        return Ok(true);
    }

    Ok(read_user_reg_hid_override(bottle_path)?.as_deref() == Some(HID_DLL_OVERRIDE_VALUE))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CrossoverWineTool {
    app_path: PathBuf,
    wine_path: PathBuf,
}

impl CrossoverWineTool {
    fn app_name(&self) -> String {
        self.app_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("CrossOver")
            .to_string()
    }
}

fn write_hid_override_reg_file(bottle_path: &Path) -> Result<PathBuf, DriverInstallError> {
    let reg_file_path = bottle_path.join(WINE_HID_OVERRIDE_REG_FILE_NAME);
    let contents = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n\
         [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
         \"{USER_REG_HID_OVERRIDE_NAME}\"=\"{HID_DLL_OVERRIDE_VALUE}\"\r\n"
    );
    fs::write(&reg_file_path, contents).map_err(|source| DriverInstallError::Io {
        path: reg_file_path.clone(),
        source,
    })?;
    Ok(reg_file_path)
}

fn find_crossover_wine_tool(
    context: &DriverInstallContext,
    bottle_path: &Path,
) -> Result<CrossoverWineTool, DriverInstallError> {
    let mut app_paths = context.crossover_app_paths.clone();
    if bottle_prefers_preview(bottle_path) {
        app_paths.sort_by_key(|path| !is_crossover_preview_app(path));
    } else {
        app_paths.sort_by_key(|path| is_crossover_preview_app(path));
    }

    for app_path in app_paths {
        for relative_path in [
            CROSSOVER_HOSTED_WINE_RELATIVE_PATH,
            CROSSOVER_BIN_WINE_RELATIVE_PATH,
        ] {
            let wine_path = relative_path
                .iter()
                .fold(app_path.clone(), |path, segment| path.join(segment));
            if wine_path.is_file() {
                return Ok(CrossoverWineTool {
                    app_path: app_path.clone(),
                    wine_path,
                });
            }
        }
    }

    Err(DriverInstallError::CrossoverWineNotFound {
        bottle_path: bottle_path.to_path_buf(),
    })
}

fn import_registry_file(
    tool: &CrossoverWineTool,
    bottle_path: &Path,
    reg_file_path: &Path,
) -> Result<(), DriverInstallError> {
    let bottle_name = bottle_path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(DEFAULT_STEAM_BOTTLE_NAME);
    let mut command = Command::new(&tool.wine_path);
    // CrossOver resolves `--bottle` names against its own configured bottles
    // directory, which can differ from the directory CrossPuck discovered the
    // bottle in (for example when bottles live on an external drive). Pin
    // CX_BOTTLE_PATH, which CrossOver honors first, to the discovered bottle's
    // parent so the import targets exactly the bottle that was inspected.
    if let Some(bottles_dir) = bottle_path.parent() {
        command.env("CX_BOTTLE_PATH", bottles_dir);
    }
    let output = command
        .arg("--bottle")
        .arg(bottle_name)
        .arg("--no-gui")
        .arg("regedit")
        .arg("/S")
        .arg(reg_file_path)
        .output()
        .map_err(|source| DriverInstallError::Io {
            path: tool.wine_path.clone(),
            source,
        })?;

    if output.status.success() {
        return Ok(());
    }

    Err(DriverInstallError::RegistryImportFailed {
        wine_path: tool.wine_path.clone(),
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn bottle_prefers_preview(bottle_path: &Path) -> bool {
    let config_path = bottle_path.join(CX_BOTTLE_CONF_FILE_NAME);
    let Ok(config) = fs::read_to_string(config_path) else {
        return false;
    };
    config.lines().any(|line| {
        let line = line.trim();
        line.starts_with("\"Preview\"") && line.rsplit('"').nth(1) == Some("1")
    })
}

fn is_crossover_preview_app(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("CrossOver Preview.app"))
}

fn user_reg_path(bottle_path: &Path) -> PathBuf {
    bottle_path.join(USER_REG_FILE_NAME)
}

fn read_user_reg_hid_override(bottle_path: &Path) -> Result<Option<String>, DriverInstallError> {
    let path = user_reg_path(bottle_path);
    let text = read_user_reg(&path)?;
    Ok(find_user_reg_hid_override(&text))
}

fn read_user_reg(path: &Path) -> Result<String, DriverInstallError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Ok("WINE REGISTRY Version 2\n\n".to_string())
        }
        Err(source) => Err(DriverInstallError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn find_user_reg_hid_override(text: &str) -> Option<String> {
    for line in user_reg_dll_override_lines(text) {
        if let Some(value) = parse_user_reg_sz_value(line, USER_REG_HID_OVERRIDE_NAME) {
            return Some(value);
        }
    }
    None
}

fn user_reg_dll_override_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut in_section = false;
    text.lines().filter(move |line| {
        if line.starts_with('[') {
            in_section = line.starts_with(USER_REG_DLL_OVERRIDES_SECTION);
            return false;
        }
        in_section
    })
}

fn parse_user_reg_sz_value(line: &str, expected_name: &str) -> Option<String> {
    let name = parse_user_reg_value_name(line)?;
    if name != expected_name {
        return None;
    }
    let value_start = line.find("=\"")? + 2;
    let value_end = line.rfind('"')?;
    (value_start <= value_end).then(|| unescape_user_reg_string(&line[value_start..value_end]))
}

fn parse_user_reg_value_name(line: &str) -> Option<String> {
    let line = line.trim_start();
    if !line.starts_with('"') {
        return None;
    }
    let name_end = line[1..].find('"')? + 1;
    Some(unescape_user_reg_string(&line[1..name_end]))
}

fn unescape_user_reg_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                output.push(next);
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn default_backup_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn default_repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_crossover_bottles_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    Some(
        CROSSOVER_BOTTLES_RELATIVE_PATH
            .iter()
            .fold(home, |path, segment| path.join(segment)),
    )
}

fn default_crossover_app_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from(CROSSOVER_APP_PATH),
        PathBuf::from(CROSSOVER_PREVIEW_APP_PATH),
    ]
}

impl fmt::Display for DriverInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundle(error) => write!(f, "{error}"),
            Self::BottleNotFound => f.write_str("Steam bottle not found"),
            Self::SteamExeNotFound { bottle_path } => {
                write!(f, "Steam.exe not found in {}", bottle_path.display())
            }
            Self::System32TargetRefused(path) => write!(
                f,
                "refusing to install CrossPuck driver into System32: {}",
                path.display()
            ),
            Self::VerificationFailed { expected, actual } => write!(
                f,
                "installed driver digest mismatch: expected {expected}, got {actual}"
            ),
            Self::CrossoverWineNotFound { bottle_path } => write!(
                f,
                "CrossOver wine command not found for Steam bottle at {}",
                bottle_path.display()
            ),
            Self::RegistryImportFailed {
                wine_path,
                status,
                stdout,
                stderr,
            } => write!(
                f,
                "failed to import Wine override with {} (status {}): {}{}{}",
                wine_path.display(),
                status
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "terminated".to_string()),
                stderr,
                if stdout.is_empty() || stderr.is_empty() {
                    ""
                } else {
                    " / "
                },
                stdout
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for DriverInstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bundle(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{DRIVER_DLL_NAME, DRIVER_MANIFEST_NAME, GUEST_DRIVER_DIR};
    use std::os::unix::fs::PermissionsExt;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "crosspuck-install-test-{}-{id}-{name}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn status_uses_explicit_bottle_path_first() {
        let dir = TestDir::new("explicit-bottle");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let explicit_bottle = write_bottle_with_steam(dir.path(), "Explicit");
        let default_bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        fs::write(
            default_bottle.join("drive_c/Program Files/Steam/hid.dll"),
            b"old",
        )
        .unwrap();

        let status = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources),
            bottle_path: Some(explicit_bottle.clone()),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            ..DriverInstallContext::default()
        });

        assert_eq!(status.state, DriverInstallState::NotInstalled);
        assert_eq!(
            status.bottle_path.as_deref(),
            Some(explicit_bottle.as_path())
        );
        assert_eq!(status.action_title, "Install Steam Driver...");
        assert!(status.action_enabled);
    }

    #[test]
    fn status_reports_already_installed_and_allows_repair() {
        let dir = TestDir::new("installed");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::write(steam_dir.join("hid.dll"), b"embedded").unwrap();
        write_user_reg_with_overrides(&bottle, &[("hid", HID_DLL_OVERRIDE_VALUE)]);

        let status = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            ..DriverInstallContext::default()
        });

        assert_eq!(status.state, DriverInstallState::Installed);
        assert_eq!(status.status_title, "Driver: Already installed.");
        assert_eq!(status.action_title, "Repair Steam Driver...");
        assert!(status.action_enabled);
    }

    #[test]
    fn status_reports_repair_when_registry_override_is_missing() {
        let dir = TestDir::new("registry-status-missing");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::write(steam_dir.join("hid.dll"), b"embedded").unwrap();
        write_user_reg_with_overrides(&bottle, &[("msi", "builtin")]);

        let status = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            ..DriverInstallContext::default()
        });

        assert_eq!(status.state, DriverInstallState::UpdateAvailable);
        assert_eq!(status.status_title, "Driver: Wine override missing.");
        assert_eq!(status.action_title, "Repair Steam Driver...");
        assert!(status.action_enabled);
    }

    #[test]
    fn status_reports_update_available_for_digest_mismatch() {
        let dir = TestDir::new("update");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        fs::write(
            bottle.join("drive_c/Program Files/Steam/hid.dll"),
            b"previous",
        )
        .unwrap();

        let status = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            ..DriverInstallContext::default()
        });

        assert_eq!(status.state, DriverInstallState::UpdateAvailable);
        assert_eq!(status.action_title, "Update Steam Driver...");
        assert!(status.action_enabled);
    }

    #[test]
    fn status_reports_missing_bottle_and_missing_steam() {
        let dir = TestDir::new("missing");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let missing_bottle = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources.clone()),
            crossover_bottles_dir: Some(dir.path().join("Bottles")),
            ..DriverInstallContext::default()
        });
        assert_eq!(missing_bottle.state, DriverInstallState::BottleNotFound);

        let bottle = dir.path().join(DEFAULT_STEAM_BOTTLE_NAME);
        fs::create_dir_all(bottle.join("drive_c")).unwrap();
        let missing_steam = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            ..DriverInstallContext::default()
        });
        assert_eq!(missing_steam.state, DriverInstallState::SteamExeNotFound);
    }

    #[test]
    fn install_backs_up_existing_driver_and_atomically_copies_embedded_driver() {
        let dir = TestDir::new("install");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::write(steam_dir.join("hid.dll"), b"previous").unwrap();

        let result = install_driver_with_timestamp(
            &DriverInstallContext {
                resources_dir: Some(resources),
                crossover_bottles_dir: Some(dir.path().to_path_buf()),
                manage_wine_registry: false,
                ..DriverInstallContext::default()
            },
            "20260526-120000",
        )
        .unwrap();

        let backup_path = steam_dir.join("crosspuck-backups/hid.dll.20260526-120000");
        assert_eq!(result.backup_path.as_deref(), Some(backup_path.as_path()));
        assert_eq!(fs::read(&backup_path).unwrap(), b"previous");
        assert_eq!(fs::read(&result.target_dll).unwrap(), b"embedded");
        assert_eq!(
            result.installed_sha256,
            bundle::sha256_file(&result.target_dll).unwrap()
        );
    }

    #[test]
    fn install_noops_when_digest_already_matches() {
        let dir = TestDir::new("noop");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::write(steam_dir.join("hid.dll"), b"embedded").unwrap();

        let result = install_driver_with_timestamp(
            &DriverInstallContext {
                resources_dir: Some(resources),
                crossover_bottles_dir: Some(dir.path().to_path_buf()),
                manage_wine_registry: false,
                ..DriverInstallContext::default()
            },
            "20260526-120000",
        )
        .unwrap();

        assert_eq!(result.backup_path, None);
        assert!(!steam_dir.join("crosspuck-backups").exists());
    }

    #[test]
    fn install_imports_generated_registry_override() {
        let dir = TestDir::new("registry-install");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let (crossover_app, wine_log) = write_fake_crossover_app(dir.path(), "CrossOver.app");
        write_user_reg_with_overrides(&bottle, &[("msi", "builtin"), ("hid", "builtin")]);

        let result = install_driver_with_timestamp(
            &DriverInstallContext {
                resources_dir: Some(resources),
                crossover_bottles_dir: Some(dir.path().to_path_buf()),
                crossover_app_paths: vec![crossover_app],
                manage_wine_registry: true,
                ..DriverInstallContext::default()
            },
            "20260526-120000",
        )
        .unwrap();

        assert_eq!(result.registry_targets.len(), 1);
        assert!(result.registry_targets[0].starts_with("CrossOver.app: "));
        assert_eq!(
            read_user_reg_hid_override(&bottle).unwrap(),
            Some(HID_DLL_OVERRIDE_VALUE.to_string())
        );
        assert!(fs::read_to_string(user_reg_path(&bottle))
            .unwrap()
            .contains("\"msi\"=\"builtin\""));
        let reg_file_path = bottle.join(WINE_HID_OVERRIDE_REG_FILE_NAME);
        let reg_file = fs::read_to_string(&reg_file_path).unwrap();
        assert!(reg_file.contains("[HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]"));
        assert!(reg_file.contains("\"hid\"=\"native,builtin\""));
        let log = fs::read_to_string(wine_log).unwrap();
        assert!(log.contains("--bottle Steam --no-gui regedit /S"));
        assert!(log.contains(reg_file_path.to_str().unwrap()));
        assert!(log.contains(&format!("CX_BOTTLE_PATH={}", dir.path().display())));
    }

    #[test]
    fn install_uses_preview_crossover_for_preview_bottle() {
        let dir = TestDir::new("registry-preview");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let (stable_app, stable_log) = write_fake_crossover_app(dir.path(), "CrossOver.app");
        let (preview_app, preview_log) =
            write_fake_crossover_app(dir.path(), "CrossOver Preview.app");
        fs::write(
            bottle.join(CX_BOTTLE_CONF_FILE_NAME),
            "\"Preview\" = \"1\"\n",
        )
        .unwrap();
        write_user_reg_with_overrides(&bottle, &[("msi", "builtin")]);

        install_driver_with_timestamp(
            &DriverInstallContext {
                resources_dir: Some(resources),
                crossover_bottles_dir: Some(dir.path().to_path_buf()),
                crossover_app_paths: vec![stable_app, preview_app],
                manage_wine_registry: true,
                ..DriverInstallContext::default()
            },
            "20260526-120000",
        )
        .unwrap();

        assert_eq!(
            read_user_reg_hid_override(&bottle).unwrap(),
            Some(HID_DLL_OVERRIDE_VALUE.to_string())
        );
        assert!(!stable_log.exists());
        assert!(fs::read_to_string(preview_log)
            .unwrap()
            .contains("--bottle Steam --no-gui regedit /S"));
    }

    #[test]
    fn uninstall_removes_driver_without_changing_registry() {
        let dir = TestDir::new("registry-uninstall");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::write(steam_dir.join("hid.dll"), b"embedded").unwrap();
        write_user_reg_with_overrides(
            &bottle,
            &[("msi", "builtin"), ("hid", HID_DLL_OVERRIDE_VALUE)],
        );
        let before = fs::read_to_string(user_reg_path(&bottle)).unwrap();

        let result = uninstall_driver(&DriverInstallContext {
            resources_dir: Some(resources),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            manage_wine_registry: true,
            ..DriverInstallContext::default()
        })
        .unwrap();

        assert!(result.removed_driver);
        assert!(!result.target_dll.exists());
        assert!(result.registry_targets.is_empty());
        assert_eq!(fs::read_to_string(user_reg_path(&bottle)).unwrap(), before);
    }

    #[test]
    fn refuses_system32_install_target() {
        let target =
            install_target_for_steam_dir("/tmp/bottle/drive_c/windows/system32").unwrap_err();

        assert!(matches!(
            target,
            DriverInstallError::System32TargetRefused(_)
        ));
    }

    fn write_embedded_driver(root: &Path, bytes: &[u8]) -> PathBuf {
        let resources = root.join("Resources");
        let guest_driver_dir = resources.join(GUEST_DRIVER_DIR);
        fs::create_dir_all(&guest_driver_dir).unwrap();
        let dll_path = guest_driver_dir.join(DRIVER_DLL_NAME);
        fs::write(&dll_path, bytes).unwrap();
        let digest = bundle::sha256_file(&dll_path).unwrap();
        fs::write(
            guest_driver_dir.join(DRIVER_MANIFEST_NAME),
            format!(
                r#"{{
  "name": "crosspuck-driver",
  "dll_name": "hid.dll",
  "target": "x86_64-pc-windows-gnu",
  "profile": "release",
  "sha256": "{digest}",
  "size": {}
}}"#,
                bytes.len()
            ),
        )
        .unwrap();
        resources
    }

    fn write_bottle_with_steam(root: &Path, bottle_name: &str) -> PathBuf {
        let bottle = root.join(bottle_name);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::create_dir_all(&steam_dir).unwrap();
        fs::write(steam_dir.join("Steam.exe"), b"").unwrap();
        bottle
    }

    fn write_fake_crossover_app(root: &Path, app_name: &str) -> (PathBuf, PathBuf) {
        let app_path = root.join(app_name);
        let wine_path = CROSSOVER_HOSTED_WINE_RELATIVE_PATH
            .iter()
            .fold(app_path.clone(), |path, segment| path.join(segment));
        fs::create_dir_all(wine_path.parent().unwrap()).unwrap();
        let log_path = root.join(format!(
            "{}.log",
            app_name
                .trim_end_matches(".app")
                .replace(' ', "-")
                .to_ascii_lowercase()
        ));
        fs::write(
            &wine_path,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> {0}
printf 'CX_BOTTLE_PATH=%s\n' "${{CX_BOTTLE_PATH:-}}" >> {0}
last=''
for arg in "$@"; do
  last="$arg"
done
bottle_dir=$(dirname "$last")
cat > "$bottle_dir/user.reg" <<'EOF'
WINE REGISTRY Version 2

[Software\\Wine\\DllOverrides] 1
#time=1
"msi"="builtin"
"hid"="native,builtin"

[Software\\Wine\\Other] 1
"preserved"="yes"
EOF
"#,
                shell_quote(&log_path)
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&wine_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wine_path, permissions).unwrap();
        (app_path, log_path)
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    fn write_user_reg_with_overrides(bottle: &Path, overrides: &[(&str, &str)]) {
        let mut lines = vec![
            "WINE REGISTRY Version 2".to_string(),
            String::new(),
            "[Software\\\\Wine\\\\DllOverrides] 1".to_string(),
            "#time=1".to_string(),
        ];
        for (name, value) in overrides {
            lines.push(format!("\"{name}\"=\"{value}\""));
        }
        lines.push(String::new());
        lines.push("[Software\\\\Wine\\\\Other] 1".to_string());
        lines.push("\"preserved\"=\"yes\"".to_string());
        fs::write(user_reg_path(bottle), lines.join("\n")).unwrap();
    }
}
