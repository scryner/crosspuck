use crate::bundle::{self, BundleError, EmbeddedDriver};
use std::cmp::Ordering;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const BOTTLE_PATH_ENV: &str = "CROSSPUCK_BOTTLE_PATH";
const EMBEDDED_DRIVER_PATH_ENV: &str = "CROSSPUCK_EMBEDDED_DRIVER_PATH";
const DEFAULT_STEAM_BOTTLE_NAME: &str = "Steam";
const DRIVER_TARGET_RELATIVE_PATH: &[&str] =
    &["target", "x86_64-pc-windows-gnu", "release", "hid.dll"];
const CROSSOVER_BOTTLES_RELATIVE_PATH: &[&str] =
    &["Library", "Application Support", "CrossOver", "Bottles"];

#[derive(Clone, Debug, Default)]
pub struct DriverInstallContext {
    pub resources_dir: Option<PathBuf>,
    pub embedded_driver_path: Option<PathBuf>,
    pub repo_root: Option<PathBuf>,
    pub bottle_path: Option<PathBuf>,
    pub crossover_bottles_dir: Option<PathBuf>,
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
}

#[derive(Debug)]
pub enum DriverInstallError {
    Bundle(BundleError),
    BottleNotFound,
    SteamExeNotFound { bottle_path: PathBuf },
    System32TargetRefused(PathBuf),
    VerificationFailed { expected: String, actual: String },
    Io { path: PathBuf, source: io::Error },
}

impl DriverInstallContext {
    pub fn from_environment(resources_dir: Option<PathBuf>) -> Self {
        Self {
            resources_dir,
            embedded_driver_path: env::var_os(EMBEDDED_DRIVER_PATH_ENV).map(PathBuf::from),
            repo_root: Some(default_repo_root()),
            bottle_path: env::var_os(BOTTLE_PATH_ENV).map(PathBuf::from),
            crossover_bottles_dir: default_crossover_bottles_dir(),
            allow_development_fallbacks: cfg!(debug_assertions),
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
            return Ok(DriverInstallResult {
                target_dll,
                backup_path: None,
                installed_sha256,
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

    Ok(DriverInstallResult {
        target_dll,
        backup_path,
        installed_sha256,
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
    let (state, status_title, action_title, action_enabled) =
        if installed_sha256 == embedded_driver.actual_sha256 {
            (
                DriverInstallState::Installed,
                "Driver: Already installed.",
                "Already installed.",
                false,
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
    fn status_reports_already_installed_and_disables_action() {
        let dir = TestDir::new("installed");
        let resources = write_embedded_driver(dir.path(), b"embedded");
        let bottle = write_bottle_with_steam(dir.path(), DEFAULT_STEAM_BOTTLE_NAME);
        let steam_dir = bottle.join("drive_c/Program Files/Steam");
        fs::write(steam_dir.join("hid.dll"), b"embedded").unwrap();

        let status = check_driver_install_status(&DriverInstallContext {
            resources_dir: Some(resources),
            crossover_bottles_dir: Some(dir.path().to_path_buf()),
            ..DriverInstallContext::default()
        });

        assert_eq!(status.state, DriverInstallState::Installed);
        assert_eq!(status.status_title, "Driver: Already installed.");
        assert_eq!(status.action_title, "Already installed.");
        assert!(!status.action_enabled);
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
                ..DriverInstallContext::default()
            },
            "20260526-120000",
        )
        .unwrap();

        assert_eq!(result.backup_path, None);
        assert!(!steam_dir.join("crosspuck-backups").exists());
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
}
