#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
timestamp="$(date +%Y%m%d-%H%M%S)"

log_path="${CROSSPUCK_HOST_HID_LOG:-$repo_root/captures/native_feedback_${timestamp}.jsonl}"
stdout_log="${CROSSPUCK_STEAM_STDOUT_LOG:-${log_path%.jsonl}.stdout.log}"
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
    echo "$label is already running. Stop it before starting this capture:" >&2
    echo "$matches" >&2
    exit 1
  fi
}

now_ms() {
  perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'
}

write_marker() {
  local scenario="$1"
  local phase="$2"
  local description="${3:-}"
  printf '{"type":"marker","unix_ms":%s,"scenario":"%s","phase":"%s","description":"%s"}\n' \
    "$(now_ms)" "$scenario" "$phase" "$description" >> "$log_path"
}

run_window() {
  local scenario="$1"
  local seconds="$2"
  local prompt="$3"

  echo
  echo "$prompt"
  read -r -p "준비되면 Enter를 누르십시오. marker start 후 ${seconds}s 동안 캡처합니다. "
  write_marker "$scenario" "start" "$prompt"
  echo "캡처 중: $scenario (${seconds}s)"
  sleep "$seconds"
  write_marker "$scenario" "end" "$prompt"
  echo "완료: $scenario"
}

mkdir -p "$(dirname "$log_path")"
mkdir -p "$(dirname "$stdout_log")"
: > "$log_path"
: > "$stdout_log"

probe="$("$script_dir/build.sh")"

echo "Probe:  $probe"
echo "Log:    $log_path"
echo "Stdout: $stdout_log"
echo "Steam:  $steam_bin"
echo "Cwd:    $steam_dir"
echo
echo "Steam이 이미 실행 중이면 완전히 종료한 뒤 다시 실행하십시오."
echo "이 스크립트는 input report 로그를 기본 비활성화하고 host->controller feedback 경로를 JSONL로 캡처합니다."
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
export CROSSPUCK_HOST_HID_JSONL="${CROSSPUCK_HOST_HID_JSONL:-1}"
export CROSSPUCK_HOST_HID_LOG_LOAD="${CROSSPUCK_HOST_HID_LOG_LOAD:-0}"
export CROSSPUCK_HOST_HID_LOG_INPUT="${CROSSPUCK_HOST_HID_LOG_INPUT:-0}"
export CROSSPUCK_HOST_HID_LOG_GET="${CROSSPUCK_HOST_HID_LOG_GET:-1}"
export CROSSPUCK_HOST_HID_LOG_VALUE="${CROSSPUCK_HOST_HID_LOG_VALUE:-1}"
export DYLD_INSERT_LIBRARIES="$probe${DYLD_INSERT_LIBRARIES:+:$DYLD_INSERT_LIBRARIES}"

write_marker "capture" "start" "native Steam feedback capture"
(
  cd "$steam_dir"
  exec "$steam_bin"
) > "$stdout_log" 2>&1 &
steam_pid=$!
echo "Steam started with pid=$steam_pid"

sleep 2
if ! kill -0 "$steam_pid" 2>/dev/null; then
  write_marker "capture" "end" "steam exited during startup"
  echo
  echo "Steam exited during startup. Last stdout/stderr lines:"
  tail -80 "$stdout_log"
  exit 1
fi

trap 'write_marker "capture" "end" "script interrupted"; echo; echo "중단됨. Steam은 직접 종료하십시오."; exit 130' INT TERM

echo
read -r -p "native Steam에서 컨트롤러가 연결된 상태가 되면 Enter를 누르십시오. "
write_marker "steam_controller_connected" "point" "controller connected in native Steam"

run_window \
  "left_touchpad_haptic" \
  5 \
  "컨트롤러의 왼쪽 터치패드를 5초간 문지르십시오. 햅틱은 패드 내부 로직일 수 있습니다."

run_window \
  "right_touchpad_haptic" \
  5 \
  "컨트롤러의 오른쪽 터치패드를 5초간 문지르십시오. 햅틱은 패드 내부 로직일 수 있습니다."

run_window \
  "steam_controller_test_ping" \
  5 \
  "Steam 컨트롤러 테스트 화면의 '핑' 버튼을 누르십시오. 진동/사운드는 ping command 또는 패드 내장 로직일 수 있습니다."

write_marker "capture" "end" "native Steam feedback capture"

echo
echo "캡처 완료: $log_path"
echo "Steam stdout: $stdout_log"
echo
echo "분석:"
cargo run -p crosspuck-cli -- --analyze-hid-probe "$log_path"
