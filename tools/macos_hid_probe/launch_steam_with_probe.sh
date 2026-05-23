#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

log_path="${CROSSPUCK_HOST_HID_LOG:-/tmp/crosspuck-host-hid.log}"
if [[ -n "${CROSSPUCK_STEAM_STDOUT_LOG:-}" ]]; then
  stdout_log="$CROSSPUCK_STEAM_STDOUT_LOG"
elif [[ "$log_path" == *.log ]]; then
  stdout_log="${log_path%.log}.stdout.log"
else
  stdout_log="${log_path}.stdout.log"
fi
steam_bin="${STEAM_OSX:-/Applications/Steam.app/Contents/MacOS/steam_osx}"

if [[ ! -x "$steam_bin" ]]; then
  echo "Steam binary not found or not executable: $steam_bin" >&2
  exit 1
fi

mkdir -p "$(dirname "$log_path")"
mkdir -p "$(dirname "$stdout_log")"
: > "$log_path"
: > "$stdout_log"
exec > >(tee -a "$stdout_log") 2>&1

probe="$(./build.sh)"

echo "Probe: $probe"
echo "Log:   $log_path"
echo "Stdout: $stdout_log"
echo "Steam: $steam_bin"
echo
echo "If Steam is already running, quit it fully before using this launcher."
echo

export CROSSPUCK_HOST_HID_LOG="$log_path"
export CROSSPUCK_HOST_HID_VID="${CROSSPUCK_HOST_HID_VID:-0x28DE}"
export CROSSPUCK_HOST_HID_PID="${CROSSPUCK_HOST_HID_PID:-0x1304}"
export CROSSPUCK_HOST_HID_MAX_BYTES="${CROSSPUCK_HOST_HID_MAX_BYTES:-256}"
export DYLD_INSERT_LIBRARIES="$probe${DYLD_INSERT_LIBRARIES:+:$DYLD_INSERT_LIBRARIES}"

exec "$steam_bin"
