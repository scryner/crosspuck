#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tools/crosspuck/install-driver.sh [--bottle NAME | --bottle-path PATH] [--driver PATH] [--steam-dir PATH] [--no-build] [--write-wine-override]

Installs the production crosspuck-driver hid.dll next to Steam.exe inside a
CrossOver bottle.

The script verifies that the Rust Windows GNU target is installed, builds the
driver with Cargo unless --no-build is provided, and then installs the resulting
DLL. It does not write guest runtime CROSSPUCK_* registry/environment settings.
Guest runtime settings must come from built-in defaults or host connection
overrides.

Options:
  --bottle NAME          CrossOver bottle name. Defaults to Steam when present.
  --bottle-path PATH     Absolute bottle path.
  --driver PATH          hid.dll path to copy. Defaults to the Cargo release output.
  --steam-dir PATH       Steam directory inside the bottle. Auto-detected from Steam.exe.
  --log-file PATH        Unix path for the driver log file. Defaults to the Steam directory.
  --no-build             Do not build the driver before copying --driver.
  --write-wine-override  Write a loader-only registry file for hid=native,builtin.
  --help                 Show this help.

The script intentionally does not install into drive_c/windows/system32 because
the driver forwards non-virtual HID calls to the real System32 hid.dll.
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
driver_target="x86_64-pc-windows-gnu"

bottle_name=""
bottle_path=""
driver_path="$repo_root/target/$driver_target/release/hid.dll"
steam_dir=""
log_file=""
no_build="0"
write_wine_override="0"

ensure_driver_target_installed() {
  if ! command -v rustup >/dev/null 2>&1; then
    cat >&2 <<EOF
rustup is required to verify the Windows target before building the driver.

Install rustup first, then add the driver target:
  rustup target add $driver_target
EOF
    exit 1
  fi

  if ! rustup target list --installed | grep -Fxq "$driver_target"; then
    cat >&2 <<EOF
Required Rust target is not installed:
  $driver_target

Install it first:
  rustup target add $driver_target
EOF
    exit 1
  fi
}

build_driver() {
  ensure_driver_target_installed
  echo "Building crosspuck-driver for $driver_target..."
  cargo build \
    --manifest-path "$repo_root/Cargo.toml" \
    -p crosspuck-driver \
    --release \
    --target "$driver_target"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bottle)
      bottle_name="${2:?missing value for --bottle}"
      shift 2
      ;;
    --bottle-path)
      bottle_path="${2:?missing value for --bottle-path}"
      shift 2
      ;;
    --driver)
      driver_path="${2:?missing value for --driver}"
      shift 2
      ;;
    --steam-dir)
      steam_dir="${2:?missing value for --steam-dir}"
      shift 2
      ;;
    --log-file)
      log_file="${2:?missing value for --log-file}"
      shift 2
      ;;
    --no-build)
      no_build="1"
      shift
      ;;
    --write-wine-override)
      write_wine_override="1"
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --log-level|--trace|--required)
      echo "Removed option: $1" >&2
      echo "Guest runtime settings are host-owned and are no longer written through bottle registry/environment values." >&2
      exit 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$bottle_path" ]]; then
  bottles_root="$HOME/Library/Application Support/CrossOver/Bottles"
  if [[ -z "$bottle_name" ]]; then
    if [[ -d "$bottles_root/Steam" ]]; then
      bottle_name="Steam"
    else
      echo "Bottle name is required. Pass --bottle NAME or --bottle-path PATH." >&2
      exit 2
    fi
  fi
  bottle_path="$bottles_root/$bottle_name"
else
  bottle_name="$(basename "$bottle_path")"
fi

if [[ ! -d "$bottle_path/drive_c" ]]; then
  echo "Invalid CrossOver bottle path: $bottle_path" >&2
  exit 1
fi

if [[ "$no_build" != "1" ]]; then
  build_driver
fi

if [[ ! -f "$driver_path" ]]; then
  cat >&2 <<EOF
Driver DLL not found:
  $driver_path

Expected build command:
  cargo build -p crosspuck-driver --release --target x86_64-pc-windows-gnu
EOF
  exit 1
fi

if [[ -z "$steam_dir" ]]; then
  steam_exe="$(find "$bottle_path/drive_c" -iname steam.exe -print -quit)"
  if [[ -z "$steam_exe" ]]; then
    echo "Could not find Steam.exe in $bottle_path/drive_c. Pass --steam-dir PATH." >&2
    exit 1
  fi
  steam_dir="$(dirname "$steam_exe")"
fi

if [[ ! -d "$steam_dir" ]]; then
  echo "Steam directory does not exist: $steam_dir" >&2
  exit 1
fi

target_dll="$steam_dir/hid.dll"
backup_dir="$steam_dir/crosspuck-backups"
timestamp="$(date +%Y%m%d-%H%M%S)"
if [[ -f "$target_dll" ]]; then
  mkdir -p "$backup_dir"
  cp -p "$target_dll" "$backup_dir/hid.dll.$timestamp"
fi
cp -f "$driver_path" "$target_dll"

if [[ -z "$log_file" ]]; then
  log_file="$steam_dir/crosspuck-driver.log"
fi
mkdir -p "$(dirname "$log_file")"
find "$steam_dir" -name crosspuck-driver.log -type f ! -path "$log_file" -delete 2>/dev/null || true
find "$bottle_path/drive_c/users" -name crosspuck-driver.log -type f -delete 2>/dev/null || true
: > "$log_file"

wine_override_file=""
if [[ "$write_wine_override" == "1" ]]; then
  wine_override_file="$bottle_path/crosspuck-wine-override.reg"
  cat > "$wine_override_file" <<EOF
Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]
"hid"="native,builtin"
EOF
fi

cat <<EOF
Installed CrossPuck driver:
  $target_dll

Driver log file:
  $log_file
EOF

if [[ -n "$wine_override_file" ]]; then
  cat <<EOF

Generated loader-only Wine override registry file:
  $wine_override_file
EOF
fi

cat <<EOF

Next:
  1. Start the macOS CrossPuck host app.
     For guest log severity override, start the host with:
       open -a CrossPuck --args --override-log-level --log-level debug
  2. Start Steam from the "$bottle_name" bottle.
  3. Watch the log:
       tail -f "$log_file"

No guest runtime CROSSPUCK_* registry/environment settings were written.
Do not copy this hid.dll into drive_c/windows/system32.
EOF
