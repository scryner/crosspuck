#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tools/crossover/install-driver.sh [--bottle NAME | --bottle-path PATH] [--driver PATH] [--steam-dir PATH]

Installs the production crosspuck-driver hid.dll next to Steam.exe inside a
CrossOver bottle and writes an optional registry file for Wine override and
trace/log settings.

Options:
  --bottle NAME       CrossOver bottle name. Defaults to Steam when present.
  --bottle-path PATH  Absolute bottle path.
  --driver PATH       hid.dll path. Defaults to target/x86_64-pc-windows-gnu/release/hid.dll.
  --steam-dir PATH    Steam directory inside the bottle. Auto-detected from Steam.exe.
  --log-file PATH     Unix path for the driver log file. Defaults to the Steam directory.
  --log-level LEVEL   Set CROSSPUCK_LOG_LEVEL. Defaults to info.
  --trace 0|1         Set CROSSPUCK_TRACE_REPORTS. Defaults to 1.
  --required 0|1      Set CROSSPUCK_HOST_BRIDGE_REQUIRED. Defaults to 1.
  --help              Show this help.

The script intentionally does not install into drive_c/windows/system32 because
the driver forwards non-virtual HID calls to the real System32 hid.dll.
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

bottle_name=""
bottle_path=""
driver_path="$repo_root/target/x86_64-pc-windows-gnu/release/hid.dll"
steam_dir=""
log_file=""
log_level="info"
trace_reports="1"
required="1"

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
    --log-level)
      log_level="${2:?missing value for --log-level}"
      shift 2
      ;;
    --trace)
      trace_reports="${2:?missing value for --trace}"
      shift 2
      ;;
    --required)
      required="${2:?missing value for --required}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
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

if [[ ! -f "$driver_path" ]]; then
  cat >&2 <<EOF
Driver DLL not found:
  $driver_path

Build it first:
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

reg_file="$bottle_path/crosspuck-driver-env.reg"
cat > "$reg_file" <<EOF
Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\\Environment]
"CROSSPUCK_HOST_BRIDGE"="1"
"CROSSPUCK_HOST_BRIDGE_REQUIRED"="$required"
"CROSSPUCK_LOG_LEVEL"="$log_level"
"CROSSPUCK_TRACE_REPORTS"="$trace_reports"
"CROSSPUCK_TRACE_REPORT_LIMIT"="2048"
"CROSSPUCK_TRACE_REPORT_MAX_BYTES"="128"
"CROSSPUCK_HOST_BRIDGE_CONNECT_TIMEOUT_MS"="1000"
"CROSSPUCK_HOST_BRIDGE_HANDSHAKE_TIMEOUT_MS"="2000"
"CROSSPUCK_HOST_BRIDGE_IO_TIMEOUT_MS"="1000"
"CROSSPUCK_HOST_BRIDGE_RECONNECT_INTERVAL_MS"="1000"

[HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]
"hid"="native,builtin"
EOF

cat <<EOF
Installed CrossPuck driver:
  $target_dll

Generated environment registry file:
  $reg_file

Driver log file:
  $log_file

Next:
  1. Optional but recommended for smoke testing: import the .reg file into the "$bottle_name" bottle with CrossOver's Run Command or regedit.
     The driver has built-in host bridge defaults, while the .reg file sets trace/log settings and the Wine hid DLL override explicitly.
  2. Start the macOS CrossPuck host app.
  3. Start Steam from the same bottle.
  4. Watch the log:
       tail -f "$log_file"

Do not copy this hid.dll into drive_c/windows/system32.
EOF
