#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"

log_path="${CROSSPUCK_HOST_HID_LOG:-/tmp/crosspuck-host-hid.log}"
if [[ -n "${CROSSPUCK_STEAM_STDOUT_LOG:-}" ]]; then
  stdout_log="$CROSSPUCK_STEAM_STDOUT_LOG"
elif [[ "$log_path" == *.log ]]; then
  stdout_log="${log_path%.log}.stdout.log"
else
  stdout_log="${log_path}.stdout.log"
fi
steam_appbundle_bin="$HOME/Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS/steam_osx"
steam_app_bin="/Applications/Steam.app/Contents/MacOS/steam_osx"
if [[ -n "${STEAM_OSX:-}" ]]; then
  steam_bin="$STEAM_OSX"
elif [[ -x "$steam_appbundle_bin" ]]; then
  steam_bin="$steam_appbundle_bin"
else
  steam_bin="$steam_app_bin"
fi

if [[ ! -x "$steam_bin" ]]; then
  echo "Steam binary not found or not executable: $steam_bin" >&2
  exit 1
fi
steam_dir="$(cd "$(dirname "$steam_bin")" && pwd)"

process_matches() {
  local pattern="$1"
  ps -axo pid=,command= | awk -v pattern="$pattern" -v self="$$" '
    {
      pid = $1
      line = $0
      sub(/^[[:space:]]*[0-9]+[[:space:]]*/, "", line)
      if (pid == self) next
      if (line ~ /awk -v pattern/) next
      if (line ~ /ps -axo/) next
      if (line ~ /\.codex\/computer-use/) next
      if (line ~ pattern) print pid " " line
    }'
}

abort_if_matches() {
  local label="$1"
  local pattern="$2"
  local matches
  matches="$(process_matches "$pattern")"
  if [[ -n "$matches" ]]; then
    echo "$label is already running. Stop it before starting this launcher:" >&2
    echo "$matches" >&2
    exit 1
  fi
}

mkdir -p "$(dirname "$log_path")"
mkdir -p "$(dirname "$stdout_log")"
: > "$log_path"
: > "$stdout_log"
exec > >(tee -a "$stdout_log") 2>&1

probe="$("$script_dir/build.sh")"

echo "Probe: $probe"
echo "Log:   $log_path"
echo "Stdout: $stdout_log"
echo "Steam: $steam_bin"
echo "Cwd:   $steam_dir"
echo
echo "If Steam is already running, quit it fully before using this launcher."
echo

abort_if_matches \
  "crosspuck-host HID capture" \
  '(^|/)(crosspuck-host)( |$)|target/debug/crosspuck-host'
if [[ "${CROSSPUCK_ALLOW_RUNNING_STEAM:-0}" != "1" ]]; then
  abort_if_matches \
    "Steam" \
    'Steam\.app/.*/steam_osx|Steam\.AppBundle/.*/steam_osx|Steam Helper|Steam\.AppBundle/.*/ipcserver'
fi

export CROSSPUCK_HOST_HID_LOG="$log_path"
export CROSSPUCK_HOST_HID_VID="${CROSSPUCK_HOST_HID_VID:-0x28DE}"
export CROSSPUCK_HOST_HID_PID="${CROSSPUCK_HOST_HID_PID:-0x1304}"
export CROSSPUCK_HOST_HID_MAX_BYTES="${CROSSPUCK_HOST_HID_MAX_BYTES:-256}"
export CROSSPUCK_HOST_HID_LOG_LOAD="${CROSSPUCK_HOST_HID_LOG_LOAD:-0}"
export DYLD_INSERT_LIBRARIES="$probe${DYLD_INSERT_LIBRARIES:+:$DYLD_INSERT_LIBRARIES}"

cd "$steam_dir"
exec "$steam_bin"
