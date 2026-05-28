# CrossPuck Tools

This directory contains install, smoke-test, logging, and reference-probe tools
used while developing and validating CrossPuck.

Shell entrypoints live directly under `tools/`. Deprecated CrossOver wrapper
scripts have been removed; use the current `tools/*.sh` commands below.

## CrossOver Install

Install the production guest driver next to `Steam.exe`:

```sh
tools/install-driver.sh --bottle Steam
```

Useful options:

```sh
tools/install-driver.sh \
  --bottle Steam \
  --driver target/x86_64-pc-windows-gnu/release/hid.dll \
  --no-build
```

The script copies `hid.dll`, backs up any existing app-local `hid.dll`, and
initializes `crosspuck-driver.log` next to Steam. It does not write guest
runtime `CROSSPUCK_*` registry or environment settings. Guest runtime settings
use built-in defaults unless the macOS host app sends overrides over the bridge
connection.

If CrossOver needs an explicit loader override for the app-local `hid.dll`, run
with `--write-wine-override` and import the generated loader-only registry file.
This only sets `hid=native,builtin`; runtime settings remain host-owned.

Do not copy this DLL into `drive_c/windows/system32`. The driver is intended to
be app-local and forwards non-virtual HID calls to the real system HID DLL.

## Logging

Host app logs use macOS Unified Logging.

```sh
open -a CrossPuck --args --log-level debug
open -a CrossPuck --args --override-log-level --log-level debug
CROSSPUCK_LOG_LEVEL=debug CrossPuck
```

Supported host levels are `off`, `error`, `warn`, `info`, `debug`, and `trace`.
The default is `info`.

Guest driver logs are written to `crosspuck-driver.log` next to Steam. The
guest default level is `info`. To override guest severity for a host session,
start the host with `--override-log-level --log-level <level>`.

- `info`: attach and bridge connection state.
- `error`: hook/bridge/virtual HID failures.
- `debug`: hook, discovery, and API-level diagnostic logs.
- `trace`: payload traces when enabled by host-owned diagnostic configuration.

## Smoke Test

The detailed CrossOver smoke procedure is documented here:

- `crates/crosspuck-driver/docs/crossover-smoke.md`
- `crates/crosspuck-driver/docs/crossover-smoke-ko.md`

After exercising the Steam UI, run:

```sh
tools/smoke-check.sh --bottle Steam
```

Warnings from this script are hints, not hard failures. Missing trace markers
usually mean the corresponding Steam UI path was not exercised, the host app was
not running, or debug/trace logging was not enabled.

## Development Checks

Run the main workspace checks:

```sh
cargo fmt --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Check the Windows guest target explicitly:

```sh
cargo check -p crosspuck-driver --target x86_64-pc-windows-gnu
cargo clippy -p crosspuck-driver --target x86_64-pc-windows-gnu -- -D warnings
```

When the MSVC target is available:

```sh
cargo check -p crosspuck-driver --target x86_64-pc-windows-msvc
cargo clippy -p crosspuck-driver --target x86_64-pc-windows-msvc -- -D warnings
```

## Host App Memory Profiling

Run the macOS host app through `cargo run` with the profiling feature and app
probe enabled:

```sh
tools/profile-app.sh
```

By default this writes a capture under `captures/app-profile/YYYYMMDD-HHMMSS`,
samples RSS and probe counters every 5 seconds, records `vmmap -summary` every
30 seconds, and writes a 60-second pprof flamegraph.

Useful options:

```sh
tools/profile-app.sh --dir /tmp/crosspuck-profile --cpu-seconds 120
tools/profile-app.sh --release --interval-ms 1000 --vmmap-interval-ms 10000
tools/profile-app.sh -- --override-log-level --log-level debug
```

After reproducing idle or touchpad-input scenarios, summarize the capture:

```sh
tools/profile-app-summary.sh --dir /tmp/crosspuck-profile
tools/profile-app-summary.sh --dir /tmp/crosspuck-profile --leaks
```

The summary reports RSS deltas, post-warmup deltas, CrossPuck probe counters,
the latest captured `vmmap` line, any generated CPU flamegraphs, and live
`ps`/`vmmap` data if the process is still running.

## macOS HID Reference Probe

The macOS HID reference probe is a host-side reference tracer for the native
macOS Steam client. The shell entrypoints are in `tools/`, while the interposer
source remains under `tools/macos_hid_probe`. It interposes IOKit HID report
calls and logs real feature/output report traffic plus input report callbacks
for the Valve puck (`VID=0x28DE`, `PID=0x1304` by default).

Build:

```sh
tools/build-macos-hid-probe.sh
```

Run native Steam with the probe:

```sh
tools/launch-steam-with-probe.sh
```

Run the feedback scenario capture for protocol design:

```sh
tools/capture-feedback-scenarios.sh
```

This launches native Steam with the probe, writes JSONL markers for these
manual windows, and runs the CLI analyzer at the end:

- left touchpad haptic, 5 seconds
- right touchpad haptic, 5 seconds
- Steam controller test "Ping", 5 seconds

Do not run `crosspuck-host`, `cargo run --bin crosspuck-host`, or any other
standalone HID reader during this capture. Those tools open the puck directly
and can prevent native Steam from recognizing it. The feedback capture must be
probe-only: the dylib observes HID calls inside the Steam process and does not
open the device on its own.

The launch scripts abort if native Steam or `crosspuck-host` is already
running. Quit Steam fully before starting a capture. If you intentionally need
to attach to an already-running Steam process tree, set
`CROSSPUCK_ALLOW_RUNNING_STEAM=1`, but that mode is not suitable for reference
captures.

Default log:

```text
/tmp/crosspuck-host-hid.log
```

Steam stdout/stderr is also captured. By default it is written next to the HID
probe log:

```text
/tmp/crosspuck-host-hid.stdout.log
```

Useful environment variables:

```sh
CROSSPUCK_HOST_HID_LOG=/tmp/crosspuck-host-hid.log
CROSSPUCK_STEAM_STDOUT_LOG=/tmp/crosspuck-steam-stdout.log
CROSSPUCK_HOST_HID_VID=0x28DE
CROSSPUCK_HOST_HID_PID=0x1304
CROSSPUCK_HOST_HID_MAX_BYTES=256
CROSSPUCK_HOST_HID_LOG_ALL=1
CROSSPUCK_HOST_HID_JSONL=1
CROSSPUCK_HOST_HID_LOG_LOAD=1
CROSSPUCK_HOST_HID_LOG_INPUT=0
CROSSPUCK_HOST_HID_LOG_GET=1
CROSSPUCK_HOST_HID_LOG_VALUE=1
STEAM_OSX="$HOME/Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS/steam_osx"
```

If `STEAM_OSX` is not set, the scripts prefer the installed `Steam.AppBundle`
client binary under `~/Library/Application Support/Steam` and fall back to
`/Applications/Steam.app/...`.

The launchers run Steam with its current working directory set to the selected
binary's `Contents/MacOS` directory. Probe load logging is disabled by default
(`CROSSPUCK_HOST_HID_LOG_LOAD=0`) so the injected library does not do file I/O
or symbol lookup until Steam actually calls one of the interposed HID APIs. Set
`CROSSPUCK_HOST_HID_LOG_LOAD=1` only when checking whether the dylib is loaded
into a process.

The key sequence to compare against the CrossOver guest is usually:

```text
SET type=feature report_id=...
GET request type=feature report_id=...
GET result type=feature report_id=... bytes=...
REGISTER input_report_callback ...
INPUT callback type=input report_id=... bytes=...
```

For feedback analysis, focus on JSONL records with:

```text
type=hid_probe
direction=host_to_device
phase=request
event=set_report | set_report_callback | set_value*
```

Analyze an existing JSONL capture:

```sh
cargo run -p crosspuck-cli -- --analyze-hid-probe captures/native_feedback_YYYYMMDD-HHMMSS.jsonl
```

For the controller-recognition failure, capture from native Steam startup
through the point where the UI shows the Steam Controller as connected. The
important comparison points are native `02 B4`, `01 83`, `01 F2`, `01 AE`, and
the first input reports that lead to `WIRELESS SYSTEM DEBUG` / `Got bond` in
Steam's controller log.
