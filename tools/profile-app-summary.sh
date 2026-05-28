#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tools/profile-app-summary.sh [--dir PATH | --log PATH] [options]

Summarizes a CrossPuck app profiling capture produced by tools/profile-app.sh.

Options:
  --dir PATH           Capture directory containing probe.log.
  --log PATH           Explicit probe.log path.
  --pid PID            Override PID from probe_start.
  --warmup-ticks N     Exclude the first N probe_tick rows from warmup summary.
                       Default: 2
  --leaks              Run macOS leaks against the live process.
  --help, -h           Show this help.

Examples:
  tools/profile-app-summary.sh --dir captures/app-profile/20260528-210000
  tools/profile-app-summary.sh --log /tmp/crosspuck-probe/probe.log --pid 12345
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="$(cd "$script_dir/.." && pwd)"

capture_dir=""
probe_log=""
pid=""
warmup_ticks="2"
run_leaks="0"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir)
      capture_dir="${2:?missing value for --dir}"
      shift 2
      ;;
    --log)
      probe_log="${2:?missing value for --log}"
      shift 2
      ;;
    --pid)
      pid="${2:?missing value for --pid}"
      shift 2
      ;;
    --warmup-ticks)
      warmup_ticks="${2:?missing value for --warmup-ticks}"
      shift 2
      ;;
    --leaks)
      run_leaks="1"
      shift
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

if [[ -z "$probe_log" ]]; then
  if [[ -z "$capture_dir" ]]; then
    latest_dir="$(find "$root_dir/captures/app-profile" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort | tail -n 1 || true)"
    if [[ -n "$latest_dir" ]]; then
      capture_dir="$latest_dir"
    else
      echo "No capture directory given and no captures/app-profile directory found." >&2
      usage >&2
      exit 2
    fi
  fi
  probe_log="$capture_dir/probe.log"
else
  capture_dir="$(dirname "$probe_log")"
fi

if [[ ! -f "$probe_log" ]]; then
  echo "probe.log not found: $probe_log" >&2
  exit 1
fi

if [[ -z "$pid" ]]; then
  pid="$(sed -nE 's/.*probe_start pid=([0-9]+).*/\1/p' "$probe_log" | tail -n 1)"
fi

echo "CrossPuck profiling summary"
echo "Capture: $capture_dir"
echo "Probe log: $probe_log"
if [[ -n "$pid" ]]; then
  echo "PID: $pid"
fi
echo

awk -v warmup="$warmup_ticks" '
function value(line, key,    pattern, start, rest) {
  pattern = key "="
  start = index(line, pattern)
  if (start == 0) {
    return ""
  }
  rest = substr(line, start + length(pattern))
  sub(/[^0-9-].*/, "", rest)
  return rest
}
function add_counter(line, name, key,    found) {
  found = value(line, key)
  if (found != "") {
    counter[name] += found
    if (tick > warmup) {
      warm_counter[name] += found
    }
  }
}
/probe_tick / {
  tick++
  rss = value($0, "rss_kb")
  rss_delta = value($0, "rss_delta_kb")
  if (rss != "") {
    if (first_rss == "") {
      first_rss = rss
    }
    last_rss = rss
    if (min_rss == "" || rss < min_rss) {
      min_rss = rss
    }
    if (max_rss == "" || rss > max_rss) {
      max_rss = rss
    }
    if (tick == warmup + 1) {
      warm_first_rss = rss
    }
    if (tick > warmup) {
      warm_last_rss = rss
      warm_ticks++
    }
  }
  if (rss_delta != "") {
    rss_delta_sum += rss_delta
    if (tick > warmup) {
      warm_rss_delta_sum += rss_delta
    }
  }
  add_counter($0, "ui_timer", "ui_timer")
  add_counter($0, "menu_open", "menu_open")
  add_counter($0, "menu_refresh", "menu_refresh")
  add_counter($0, "driver_status", "driver_status")
  add_counter($0, "control_frames", "control_frames")
  add_counter($0, "input_reports", "input_reports")
  add_counter($0, "hid_open_path", "hid_open_path")
  add_counter($0, "hid_interface_reopen_ok", "hid_interface_reopen")
  add_counter($0, "hid_error_reopen_ok", "hid_error_reopen")
  add_counter($0, "hid_main_refresh_ok", "hid_main_refresh")
}
END {
  if (tick == 0) {
    print "No probe_tick rows found."
    exit
  }
  printf("ticks=%d first_rss_kb=%s last_rss_kb=%s min_rss_kb=%s max_rss_kb=%s rss_delta_sum_kb=%d\n",
    tick, first_rss, last_rss, min_rss, max_rss, rss_delta_sum)
  if (warm_ticks > 0) {
    printf("after_warmup_ticks=%d warmup_ticks=%d first_rss_kb=%s last_rss_kb=%s rss_delta_sum_kb=%d\n",
      warm_ticks, warmup, warm_first_rss, warm_last_rss, warm_rss_delta_sum)
  }
  printf("counter_delta control_frames=%d input_reports=%d hid_open_path=%d hid_interface_reopen_ok=%d hid_error_reopen_ok=%d hid_main_refresh_ok=%d menu_refresh=%d driver_status=%d ui_timer=%d menu_open=%d\n",
    counter["control_frames"], counter["input_reports"], counter["hid_open_path"],
    counter["hid_interface_reopen_ok"], counter["hid_error_reopen_ok"],
    counter["hid_main_refresh_ok"], counter["menu_refresh"], counter["driver_status"],
    counter["ui_timer"], counter["menu_open"])
  if (warm_ticks > 0) {
    printf("after_warmup_counter_delta control_frames=%d input_reports=%d hid_open_path=%d hid_interface_reopen_ok=%d hid_error_reopen_ok=%d hid_main_refresh_ok=%d menu_refresh=%d driver_status=%d ui_timer=%d menu_open=%d\n",
      warm_counter["control_frames"], warm_counter["input_reports"], warm_counter["hid_open_path"],
      warm_counter["hid_interface_reopen_ok"], warm_counter["hid_error_reopen_ok"],
      warm_counter["hid_main_refresh_ok"], warm_counter["menu_refresh"], warm_counter["driver_status"],
      warm_counter["ui_timer"], warm_counter["menu_open"])
  }
}
' "$probe_log"

latest_vmmap="$(grep 'probe_vmmap ' "$probe_log" | tail -n 1 || true)"
if [[ -n "$latest_vmmap" ]]; then
  echo
  echo "Latest probe vmmap summary:"
  echo "$latest_vmmap"
fi

echo
echo "CPU flamegraphs:"
find "$capture_dir" -maxdepth 1 -type f -name 'cpu-*.svg' -print | sort || true

if [[ -n "$pid" ]] && ps -p "$pid" >/dev/null 2>&1; then
  echo
  echo "Live process:"
  ps -o pid,rss,vsz,etime,command -p "$pid"

  if command -v vmmap >/dev/null 2>&1; then
    echo
    echo "Live vmmap summary:"
    vmmap -summary "$pid" 2>/dev/null \
      | awk '
          /^[[:space:]]*Physical footprint:/ ||
          /^[[:space:]]*MALLOC_SMALL/ ||
          /^[[:space:]]*MALLOC_TINY/ ||
          /^[[:space:]]*MALLOC metadata/ ||
          /^[[:space:]]*DefaultMallocZone_/ ||
          /^[[:space:]]*TOTAL / { gsub(/[[:space:]]+/, " "); print }
        ' || true
  fi

  if [[ "$run_leaks" == "1" ]]; then
    echo
    echo "Live leaks result:"
    leaks "$pid" || true
  fi
else
  echo
  echo "Live process is not running; pass --pid for an active process if needed."
fi
