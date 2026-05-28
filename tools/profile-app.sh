#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tools/profile-app.sh [options] [-- app args...]

Runs CrossPuck with the profiling feature enabled and writes probe output to a
reproducible capture directory.

Options:
  --dir PATH                 Capture directory.
                             Default: captures/app-profile/YYYYMMDD-HHMMSS
  --release                  Run the app with cargo --release.
  --interval-ms N            RSS/counter sample interval. Default: 5000
  --vmmap-interval-ms N      vmmap summary interval. Default: 30000
                             Use 0 to disable vmmap snapshots.
  --cpu-seconds N            pprof CPU flamegraph duration. Default: 60
                             Use 0 to disable CPU profiling.
  --cpu-hz N                 pprof sample frequency. Default: 99
  --callback-pool            Enable the callback autorelease-pool experiment.
  --log-level LEVEL          Host log level via CROSSPUCK_LOG_LEVEL.
                             Default: info
  --help, -h                 Show this help.

Examples:
  tools/profile-app.sh
  tools/profile-app.sh --dir /tmp/crosspuck-profile --cpu-seconds 120
  tools/profile-app.sh --release -- --override-log-level --log-level debug

Artifacts:
  probe.log                  RSS, vmmap summaries, and CrossPuck counters.
  cpu-<pid>-<seconds>s.svg   pprof flamegraph, when --cpu-seconds is non-zero.

After reproducing a scenario, summarize the capture with:
  tools/profile-app-summary.sh --dir <capture-dir>
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="$(cd "$script_dir/.." && pwd)"
timestamp="$(date +%Y%m%d-%H%M%S)"

capture_dir="$root_dir/captures/app-profile/$timestamp"
cargo_profile_args=()
interval_ms="5000"
vmmap_interval_ms="30000"
cpu_seconds="60"
cpu_hz="99"
callback_pool="0"
log_level="${CROSSPUCK_LOG_LEVEL:-info}"
app_args=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir)
      capture_dir="${2:?missing value for --dir}"
      shift 2
      ;;
    --release)
      cargo_profile_args+=(--release)
      shift
      ;;
    --interval-ms)
      interval_ms="${2:?missing value for --interval-ms}"
      shift 2
      ;;
    --vmmap-interval-ms)
      vmmap_interval_ms="${2:?missing value for --vmmap-interval-ms}"
      shift 2
      ;;
    --cpu-seconds)
      cpu_seconds="${2:?missing value for --cpu-seconds}"
      shift 2
      ;;
    --cpu-hz)
      cpu_hz="${2:?missing value for --cpu-hz}"
      shift 2
      ;;
    --callback-pool)
      callback_pool="1"
      shift
      ;;
    --log-level)
      log_level="${2:?missing value for --log-level}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      app_args=("$@")
      break
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

mkdir -p "$capture_dir"

cat >"$capture_dir/profile-env.txt" <<EOF
CROSSPUCK_PROBE=1
CROSSPUCK_PROBE_DIR=$capture_dir
CROSSPUCK_PROBE_INTERVAL_MS=$interval_ms
CROSSPUCK_PROBE_VMMAP_INTERVAL_MS=$vmmap_interval_ms
CROSSPUCK_PROBE_CPU_SECONDS=$cpu_seconds
CROSSPUCK_PROBE_CPU_HZ=$cpu_hz
CROSSPUCK_PROBE_AUTORELEASE_POOL=$callback_pool
CROSSPUCK_LOG_LEVEL=$log_level
EOF

cat <<EOF
CrossPuck profiling run
Capture: $capture_dir
Probe log: $capture_dir/probe.log
CPU flamegraph: $capture_dir/cpu-<pid>-${cpu_seconds}s.svg

Run summary after quitting the app:
  tools/profile-app-summary.sh --dir "$capture_dir"
EOF

export CROSSPUCK_PROBE=1
export CROSSPUCK_PROBE_DIR="$capture_dir"
export CROSSPUCK_PROBE_INTERVAL_MS="$interval_ms"
export CROSSPUCK_PROBE_VMMAP_INTERVAL_MS="$vmmap_interval_ms"
export CROSSPUCK_PROBE_CPU_SECONDS="$cpu_seconds"
export CROSSPUCK_PROBE_CPU_HZ="$cpu_hz"
export CROSSPUCK_PROBE_AUTORELEASE_POOL="$callback_pool"
export CROSSPUCK_LOG_LEVEL="$log_level"

exec cargo run \
  --manifest-path "$root_dir/Cargo.toml" \
  -p crosspuck-app \
  --features profiling \
  --bin CrossPuck \
  "${cargo_profile_args[@]}" \
  -- \
  "${app_args[@]}"
