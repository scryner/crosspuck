use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const GUEST_DRIVER_DIR: &str = "GuestDriver";
pub const DRIVER_DLL_NAME: &str = "hid.dll";
pub const DRIVER_MANIFEST_NAME: &str = "manifest.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct DriverManifest {
    pub name: String,
    pub dll_name: String,
    pub target: String,
    pub profile: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedDriver {
    pub dll_path: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub manifest: DriverManifest,
    pub actual_sha256: String,
    pub actual_size: u64,
}

#[derive(Debug)]
pub enum BundleError {
    MissingDriver(PathBuf),
    MissingManifest(PathBuf),
    ManifestRead {
        path: PathBuf,
        source: io::Error,
    },
    ManifestParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    ManifestDllMismatch {
        expected: String,
        actual: String,
    },
    ManifestShaMismatch {
        expected: String,
        actual: String,
    },
    ManifestSizeMismatch {
        expected: u64,
        actual: u64,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl EmbeddedDriver {
    pub fn from_resources_dir(resources_dir: impl AsRef<Path>) -> Result<Self, BundleError> {
        let guest_driver_dir = resources_dir.as_ref().join(GUEST_DRIVER_DIR);
        Self::from_guest_driver_dir(guest_driver_dir)
    }

    pub fn from_guest_driver_dir(guest_driver_dir: impl AsRef<Path>) -> Result<Self, BundleError> {
        let guest_driver_dir = guest_driver_dir.as_ref();
        let manifest_path = guest_driver_dir.join(DRIVER_MANIFEST_NAME);
        let dll_path = guest_driver_dir.join(DRIVER_DLL_NAME);
        Self::from_manifest_and_dll(manifest_path, dll_path)
    }

    pub fn from_manifest_and_dll(
        manifest_path: impl AsRef<Path>,
        dll_path: impl AsRef<Path>,
    ) -> Result<Self, BundleError> {
        let manifest_path = manifest_path.as_ref().to_path_buf();
        let dll_path = dll_path.as_ref().to_path_buf();
        if !manifest_path.is_file() {
            return Err(BundleError::MissingManifest(manifest_path));
        }
        let manifest_bytes =
            fs::read(&manifest_path).map_err(|source| BundleError::ManifestRead {
                path: manifest_path.clone(),
                source,
            })?;
        let manifest: DriverManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|source| {
                BundleError::ManifestParse {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
        Self::from_manifest(dll_path, Some(manifest_path), manifest)
    }

    pub fn from_standalone_dll(dll_path: impl AsRef<Path>) -> Result<Self, BundleError> {
        let dll_path = dll_path.as_ref().to_path_buf();
        let actual_sha256 = sha256_file(&dll_path)?;
        let actual_size = file_size(&dll_path)?;
        Ok(Self {
            dll_path,
            manifest_path: None,
            manifest: DriverManifest {
                name: "crosspuck-driver".to_string(),
                dll_name: DRIVER_DLL_NAME.to_string(),
                target: "x86_64-pc-windows-gnu".to_string(),
                profile: "release".to_string(),
                sha256: actual_sha256.clone(),
                size: actual_size,
            },
            actual_sha256,
            actual_size,
        })
    }

    fn from_manifest(
        dll_path: PathBuf,
        manifest_path: Option<PathBuf>,
        manifest: DriverManifest,
    ) -> Result<Self, BundleError> {
        if !dll_path.is_file() {
            return Err(BundleError::MissingDriver(dll_path));
        }
        let actual_name = dll_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if manifest.dll_name != actual_name {
            return Err(BundleError::ManifestDllMismatch {
                expected: manifest.dll_name,
                actual: actual_name,
            });
        }

        let actual_sha256 = sha256_file(&dll_path)?;
        if manifest.sha256.to_ascii_lowercase() != actual_sha256 {
            return Err(BundleError::ManifestShaMismatch {
                expected: manifest.sha256,
                actual: actual_sha256,
            });
        }

        let actual_size = file_size(&dll_path)?;
        if manifest.size != actual_size {
            return Err(BundleError::ManifestSizeMismatch {
                expected: manifest.size,
                actual: actual_size,
            });
        }

        Ok(Self {
            dll_path,
            manifest_path,
            manifest,
            actual_sha256,
            actual_size,
        })
    }
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String, BundleError> {
    let path = path.as_ref();
    let mut file = fs::File::open(path).map_err(|source| BundleError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|source| BundleError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn file_size(path: impl AsRef<Path>) -> Result<u64, BundleError> {
    let path = path.as_ref();
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|source| BundleError::Io {
            path: path.to_path_buf(),
            source,
        })
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDriver(path) => write!(f, "bundled driver missing: {}", path.display()),
            Self::MissingManifest(path) => {
                write!(f, "driver manifest missing: {}", path.display())
            }
            Self::ManifestRead { path, source } => {
                write!(
                    f,
                    "failed to read driver manifest {}: {source}",
                    path.display()
                )
            }
            Self::ManifestParse { path, source } => {
                write!(
                    f,
                    "failed to parse driver manifest {}: {source}",
                    path.display()
                )
            }
            Self::ManifestDllMismatch { expected, actual } => write!(
                f,
                "driver manifest dll mismatch: expected {expected}, got {actual}"
            ),
            Self::ManifestShaMismatch { expected, actual } => write!(
                f,
                "driver manifest sha256 mismatch: expected {expected}, got {actual}"
            ),
            Self::ManifestSizeMismatch { expected, actual } => write!(
                f,
                "driver manifest size mismatch: expected {expected}, got {actual}"
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ManifestRead { source, .. } | Self::Io { source, .. } => Some(source),
            Self::ManifestParse { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "crosspuck-bundle-test-{}-{id}-{name}",
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
    fn loads_embedded_driver_manifest_and_validates_digest() {
        let dir = TestDir::new("valid");
        let resources = dir.path().join("Resources");
        let guest_driver_dir = resources.join(GUEST_DRIVER_DIR);
        fs::create_dir_all(&guest_driver_dir).unwrap();
        let dll_path = guest_driver_dir.join(DRIVER_DLL_NAME);
        fs::write(&dll_path, b"driver-bytes").unwrap();
        let digest = sha256_file(&dll_path).unwrap();
        fs::write(
            guest_driver_dir.join(DRIVER_MANIFEST_NAME),
            format!(
                r#"{{
  "name": "crosspuck-driver",
  "dll_name": "hid.dll",
  "target": "x86_64-pc-windows-gnu",
  "profile": "release",
  "sha256": "{digest}",
  "size": 12
}}"#
            ),
        )
        .unwrap();

        let embedded = EmbeddedDriver::from_resources_dir(&resources).unwrap();

        assert_eq!(embedded.dll_path, dll_path);
        assert_eq!(embedded.manifest.sha256, digest);
        assert_eq!(embedded.actual_size, 12);
    }

    #[test]
    fn rejects_manifest_digest_mismatch() {
        let dir = TestDir::new("sha-mismatch");
        let guest_driver_dir = dir.path().join(GUEST_DRIVER_DIR);
        fs::create_dir_all(&guest_driver_dir).unwrap();
        fs::write(guest_driver_dir.join(DRIVER_DLL_NAME), b"driver-bytes").unwrap();
        fs::write(
            guest_driver_dir.join(DRIVER_MANIFEST_NAME),
            r#"{
  "name": "crosspuck-driver",
  "dll_name": "hid.dll",
  "target": "x86_64-pc-windows-gnu",
  "profile": "release",
  "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
  "size": 12
}"#,
        )
        .unwrap();

        let error = EmbeddedDriver::from_guest_driver_dir(&guest_driver_dir).unwrap_err();

        assert!(matches!(error, BundleError::ManifestShaMismatch { .. }));
    }
}
