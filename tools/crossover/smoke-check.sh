#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tools/crossover/smoke-check.sh [--bottle NAME | --bottle-path PATH] [--log-file PATH]

Checks the CrossOver bottle files and scans the CrossPuck driver log for the
minimum markers expected after a Steam smoke test.
USAGE
}

bottle_name=""
bottle_path=""
log_file=""

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
    --log-file)
      log_file="${2:?missing value for --log-file}"
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
    bottle_name="Steam"
  fi
  bottle_path="$bottles_root/$bottle_name"
else
  bottle_name="$(basename "$bottle_path")"
fi

if [[ ! -d "$bottle_path/drive_c" ]]; then
  echo "FAIL bottle not found: $bottle_path"
  exit 1
fi

steam_exe="$(find "$bottle_path/drive_c" -iname steam.exe -print -quit)"
if [[ -z "$steam_exe" ]]; then
  echo "FAIL Steam.exe not found in bottle"
  exit 1
fi
steam_dir="$(dirname "$steam_exe")"
driver_dll="$steam_dir/hid.dll"
reg_file="$bottle_path/crosspuck-driver-env.reg"

if [[ -z "$log_file" ]]; then
  windows_user_dir="$(find "$bottle_path/drive_c/users" -mindepth 1 -maxdepth 1 -type d \
    ! -name Public ! -name Default ! -name 'All Users' -print -quit 2>/dev/null || true)"
  windows_user="crossover"
  if [[ -n "$windows_user_dir" ]]; then
    windows_user="$(basename "$windows_user_dir")"
  fi
  log_file="$bottle_path/drive_c/users/$windows_user/Temp/crosspuck-driver.log"
fi

failures=0
check_file() {
  local label="$1"
  local path="$2"
  if [[ -f "$path" ]]; then
    echo "OK   $label: $path"
  else
    echo "FAIL $label missing: $path"
    failures=$((failures + 1))
  fi
}

check_log() {
  local label="$1"
  local pattern="$2"
  if [[ -f "$log_file" ]] && grep -Eq "$pattern" "$log_file"; then
    echo "OK   log marker: $label"
  else
    echo "WARN log marker missing: $label"
  fi
}

check_file "installed driver" "$driver_dll"
check_file "env registry" "$reg_file"
check_file "driver log" "$log_file"

check_log "DLL attach" "crosspuck-driver attached"
check_log "hook install" "hook install ok"
check_log "host bridge result" "startup bridge connect (ok|failed)"
check_log "HID discovery or caps" "HidP_GetCaps|SetupDi|CreateFile"
check_log "input/feature/write trace" "ReadFile|HidD_GetInputReport|HidD_GetFeature|HidD_SetFeature|HidD_SetOutputReport|WriteFile"
check_log "DeviceIoControl trace" "DeviceIoControl"

if [[ "$failures" -gt 0 ]]; then
  exit 1
fi

echo
echo "Bottle: $bottle_name"
echo "Steam:  $steam_exe"
echo "Log:    $log_file"
echo
echo "WARN markers are smoke hints, not hard failures. Missing trace markers usually mean Steam did not load the DLL, env vars were not imported, or the relevant UI path was not exercised yet."
