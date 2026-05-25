# CrossOver Guest Driver Smoke Test

This procedure verifies the production guest-side `hid.dll` in a CrossOver
Steam bottle. It is intentionally semi-automated: scripts handle file placement
and log checks, while Steam UI confirmation remains manual.

## Scope

The smoke test checks:

- Steam loads CrossPuck's `hid.dll` from the Steam application directory.
- The DLL installs hooks and connects to the macOS host bridge.
- SetupAPI/HID discovery reaches the virtual Steam Controller Puck profiles.
- SDL hidapi discovery/open/read/feature/write reaches the same virtual
  profiles when Steam uses SDL's HID path.
- `ReadFile`, `HidD_GetInputReport`, feature/output/write paths produce trace
  markers when exercised.
- Host disconnect and reconnect do not crash the Steam process.

The test does not prove long-session stability. Keep the 5 minute idle and
reconnect test as a separate pass before calling the driver release-ready.

## Prerequisites

- A CrossOver bottle with Steam installed.
- The macOS CrossPuck host app built and able to see the controller.
- A GNU-target Windows driver DLL:

```sh
cargo build -p crosspuck-driver --release --target x86_64-pc-windows-gnu
```

The expected output is:

```text
target/x86_64-pc-windows-gnu/release/hid.dll
```

`x86_64-pc-windows-msvc` is still useful for Windows-native builds and target
checks, but it requires MSVC `link.exe`. The GNU target is the practical local
CrossOver smoke build target on macOS.

## Install Into The Bottle

Install next to `Steam.exe`, not into `drive_c/windows/system32`.

```sh
tools/crossover/install-driver.sh --bottle Steam
```

Optional flags:

```sh
tools/crossover/install-driver.sh \
  --bottle Steam \
  --driver target/x86_64-pc-windows-gnu/release/hid.dll \
  --log-level trace \
  --trace 1 \
  --required 1
```

The script:

- copies `hid.dll` into the detected Steam directory,
- backs up an existing local `hid.dll` under `crosspuck-backups/`,
- creates `crosspuck-driver-env.reg` in the bottle root,
- initializes `crosspuck-driver.log` in the Steam directory.

## Import DLL Override And Environment Variables

Import the generated registry file into the same bottle:

```text
<Bottle>/crosspuck-driver-env.reg
```

This step is recommended for smoke testing, but the production driver now has
safe built-in defaults when the registry/env values are missing:

```text
CROSSPUCK_HOST_BRIDGE=1
CROSSPUCK_HOST_BRIDGE_REQUIRED=1
CROSSPUCK_LOG_LEVEL=info
CROSSPUCK_TRACE_REPORTS=0
CROSSPUCK_HOST_BRIDGE_CONNECT_TIMEOUT_MS=1000
CROSSPUCK_HOST_BRIDGE_HANDSHAKE_TIMEOUT_MS=2000
```

When `CROSSPUCK_HOST_BRIDGE_IO_TIMEOUT_MS` is unset, the driver uses
operation-specific low-latency timeouts: `WRITE`/`SET_OUTPUT` 20ms,
`SET_FEATURE` 50ms, and `GET_FEATURE` 100ms. With those defaults, discovery
should work without importing this registry file as long as the host app is
running before Steam starts.

Importing the `.reg` file is still useful for two reasons:

- it sets Wine's `hid` DLL override to prefer the native DLL copied next to
  `Steam.exe`,
- it sets the `CROSSPUCK_*` bridge/trace environment variables used by the
  guest driver,
- it removes any older global `CROSSPUCK_HOST_BRIDGE_IO_TIMEOUT_MS` registry
  value so the per-operation defaults apply.

The registry file sets this DLL override:

```text
HKCU\Software\Wine\DllOverrides
hid = native,builtin
```

`native,builtin` lets Steam load CrossPuck's app-local `hid.dll` first while
still allowing the driver to fall back to Wine's builtin `hid` implementation
for non-virtual HID calls.

The registry file also sets:

```text
CROSSPUCK_HOST_BRIDGE=1
CROSSPUCK_HOST_BRIDGE_REQUIRED=1
CROSSPUCK_LOG_LEVEL=info
CROSSPUCK_TRACE_REPORTS=0
```

Set `CROSSPUCK_LOG_LEVEL=debug` for hook/discovery diagnostics, or
`CROSSPUCK_LOG_LEVEL=trace` with `CROSSPUCK_TRACE_REPORTS=1` for payload traces.

One practical CrossOver path:

1. Open CrossOver.
2. Select the Steam bottle.
3. Use Run Command.
4. Run `regedit`.
5. Import `crosspuck-driver-env.reg`.
6. Quit Steam fully if it was already running.

The install script generates the `.reg` file but does not currently import it
automatically. Import it when you want trace logging and an explicit bottle
override record.

If CrossOver does not pick up `HKCU\Environment` immediately, restart the
bottle or CrossOver before launching Steam.

## Run The Smoke

Start log watching first:

```sh
tail -f "$HOME/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/crosspuck-driver.log"
```

Start the macOS host app and confirm it sees the controller. Then start Steam
from the CrossOver Steam bottle.

Expected early log markers:

```text
[crosspuck] crosspuck-driver attached host_bridge=true required=true trace=true
[crosspuck] startup bridge connect skipped: lazy connect enabled
```

`hook install ok` and API-level discovery lines are debug-level logs. They are
only expected when `CROSSPUCK_LOG_LEVEL=debug` or `trace`.

The host bridge connects lazily when Steam first performs HID discovery or opens
one of the synthetic paths:

```text
[crosspuck] lazy bridge connect ok reason=... identity=Live profiles=5 open_handles=0
```

When Steam's SDL path is active, these hook markers are also expected:

```text
[crosspuck] SDL3.dll load for hid hooks -> ...
[crosspuck] optional hook installed SDL3.dll!SDL_hid_enumerate
[crosspuck] optional hook installed SDL3.dll!SDL_hid_open_path
```

If the host app is not running yet, this marker can appear when Steam first
touches HID:

```text
[crosspuck] lazy bridge connect failed reason=...: ...
```

Steam should retry through the lazy reconnect path when later HID calls occur.

## Manual UI Steps

1. Open Steam in the CrossOver bottle.
2. Navigate to controller settings or the controller test UI.
3. Confirm Steam shows a connected Steam Controller/Puck-compatible device.
4. Press controller buttons and move controls.
5. Trigger a controller test action that sends feature/output/write traffic,
   such as rumble/ping if available.
6. Quit the host app while Steam is open, wait a few seconds, then start the
   host app again.
7. Repeat a small input or feature action after reconnect.

## Automated Log Check

After the UI pass, run:

```sh
tools/crossover/smoke-check.sh --bottle Steam
```

Hard failures mean the DLL or generated files are missing. Warnings mean a log
marker was not observed. Common warning causes:

- Steam did not load the local `hid.dll`.
- The registry env vars were not imported for the bottle.
- The host app was not running.
- The Steam UI path did not exercise that API yet.
- SDL hidapi was not loaded by this Steam process, in which case the Win32 HID
  markers are the relevant path.

## Success Criteria

Minimum pass:

- The driver log contains `crosspuck-driver attached`.
- At debug or trace log level, the driver log contains `hook install ok`.
- The bridge eventually logs `lazy bridge connect ok` or later trace proves
  host-backed HID calls are occurring.
- Steam does not crash during discovery.
- Steam displays the controller as connected or usable.
- Input actions produce host-backed input trace or visible Steam UI response.
- Feature/output/write actions do not fail the UI flow.
- Host app stop/start does not crash Steam and later actions recover.

## Rollback

Quit Steam fully, then remove the local DLL:

```sh
rm "$HOME/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/hid.dll"
```

If a previous local `hid.dll` existed, restore it from:

```text
<Steam dir>/crosspuck-backups/
```

Remove the environment variables from the bottle if needed by deleting them from
`HKCU\Environment` through `regedit`.
