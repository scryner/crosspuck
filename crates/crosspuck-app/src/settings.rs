//! Persistent user preferences for the CrossPuck menu bar app.
//!
//! Today this is just the CrossOver bottle selection. CrossPuck normally
//! auto-detects the Steam bottle under the default CrossOver bottles
//! directory, but bottles can live elsewhere (for example on an external
//! drive) and a machine can have several bottles with Steam installed, so the
//! menu lets the user pick the exact bottle and remembers the choice across
//! launches via macOS `UserDefaults`.
//!
//! The effective bottle is resolved in priority order:
//!   1. the `CROSSPUCK_BOTTLE_PATH` environment variable,
//!   2. the bottle chosen from the menu (persisted in `UserDefaults`),
//!   3. automatic detection under the bottles root, which itself honors the
//!      `CROSSPUCK_BOTTLES_DIR` environment variable before the default
//!      CrossOver location.

use crate::driver_install::{default_crossover_bottles_dir, BOTTLE_PATH_ENV};
use objc2::runtime::AnyObject;
use objc2_foundation::{NSString, NSUserDefaults};
use std::env;
use std::path::{Path, PathBuf};

/// Environment override for the bottles root scanned during auto-detection.
const BOTTLES_DIR_ENV: &str = "CROSSPUCK_BOTTLES_DIR";
/// `UserDefaults` key holding the user-selected bottle.
const BOTTLE_PATH_DEFAULTS_KEY: &str = "BottlePath";

/// Resolve the explicitly selected CrossOver bottle, if any (env var first,
/// then the persisted menu selection). `None` means auto-detection.
pub fn resolve_bottle_path() -> Option<PathBuf> {
    resolve_bottle_path_from(env_bottle_path(), stored_bottle_path())
}

/// The bottle persisted via the menu, if the user has chosen one.
pub fn stored_bottle_path() -> Option<PathBuf> {
    let key = NSString::from_str(BOTTLE_PATH_DEFAULTS_KEY);
    let value = NSUserDefaults::standardUserDefaults()
        .stringForKey(&key)?
        .to_string();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// Persist a user-selected bottle.
pub fn set_stored_bottle_path(path: &Path) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(BOTTLE_PATH_DEFAULTS_KEY);
    let value = NSString::from_str(&path.to_string_lossy());
    let value_obj: &AnyObject = &value;
    // SAFETY: `value_obj` is an `NSString`, a valid property-list type for
    // `UserDefaults`, and `key` is a constant string.
    unsafe { defaults.setObject_forKey(Some(value_obj), &key) };
}

/// Forget any user-selected bottle, reverting to the env var / auto-detection.
pub fn clear_stored_bottle_path() {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(BOTTLE_PATH_DEFAULTS_KEY);
    defaults.removeObjectForKey(&key);
}

/// Whether `CROSSPUCK_BOTTLE_PATH` is set. While it is, the env var wins over
/// any menu selection, so the UI disables the bottle picker to avoid silent
/// no-ops.
pub fn env_override_active() -> bool {
    env_bottle_path().is_some()
}

/// The CrossOver bottles root scanned when no explicit bottle is selected:
/// the `CROSSPUCK_BOTTLES_DIR` environment variable, then the default
/// `~/Library/Application Support/CrossOver/Bottles`.
pub fn resolve_bottles_dir() -> Option<PathBuf> {
    non_empty_env(BOTTLES_DIR_ENV).or_else(default_crossover_bottles_dir)
}

fn env_bottle_path() -> Option<PathBuf> {
    non_empty_env(BOTTLE_PATH_ENV)
}

fn non_empty_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Pure precedence logic, split out so it can be unit tested without touching
/// the environment or `UserDefaults`.
fn resolve_bottle_path_from(
    env_path: Option<PathBuf>,
    stored_path: Option<PathBuf>,
) -> Option<PathBuf> {
    env_path.or(stored_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> Option<PathBuf> {
        Some(PathBuf::from(value))
    }

    #[test]
    fn env_overrides_stored() {
        assert_eq!(
            resolve_bottle_path_from(path("/env/Steam"), path("/stored/Steam")),
            path("/env/Steam")
        );
    }

    #[test]
    fn stored_used_without_env() {
        assert_eq!(
            resolve_bottle_path_from(None, path("/stored/Steam")),
            path("/stored/Steam")
        );
    }

    #[test]
    fn none_means_auto_detection() {
        assert_eq!(resolve_bottle_path_from(None, None), None);
    }
}
