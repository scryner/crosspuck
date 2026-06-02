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
tools/install-driver.sh --bottle Steam --override-dll
```

Optional flags:

```sh
tools/install-driver.sh \
  --bottle Steam \
  --driver target/x86_64-pc-windows-gnu/release/hid.dll \
  --override-dll \
  --no-build
```

The script:

- copies `hid.dll` into the detected Steam directory,
- backs up an existing local `hid.dll` under `crosspuck-backups/`,
- initializes `crosspuck-driver.log` in the Steam directory,
- when `--override-dll` is set, writes `crosspuck-wine-override.reg` in the
  bottle and imports it with CrossOver `regedit`.

The installer does not write guest runtime `CROSSPUCK_*` registry/environment
settings. Guest runtime settings use built-in defaults unless the macOS host app
sends overrides over the bridge connection.

## Wine Loader Override

The install script generates and imports the loader-only Wine override registry
file only when run with `--override-dll`.

The generated file is kept in the same bottle:

```text
<Bottle>/crosspuck-wine-override.reg
```

The file only sets this DLL override:

```text
HKCU\Software\Wine\DllOverrides
hid = native,builtin
```

`native,builtin` lets Steam load CrossPuck's app-local `hid.dll` first while
still allowing the driver to fall back to Wine's builtin `hid` implementation
for non-virtual HID calls.

It does not set guest runtime options. Guest severity is controlled by the
override that the macOS host app sends over the bridge connection, for example:

```sh
open -a CrossPuck --args --override-log-level --log-level debug
```

Quit Steam fully if it was already running before importing the override.

## Run The Smoke

Start log watching first:

```sh
tail -f "$HOME/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/crosspuck-driver.log"
```

Start the macOS host app and confirm it sees the controller. Grant CrossPuck
Input Monitoring permission when macOS asks; if this is denied, guest bridge
handshake can fail before identity is sent. If permission was denied earlier,
enable it in System Settings and restart CrossPuck. Then start Steam from the
CrossOver Steam bottle.

Expected early log markers:

```text
[crosspuck] crosspuck-driver attached ... host_bridge=true required=true ...
```

`hook install ok` and API-level discovery lines are debug-level logs. They are
only expected when the host applies a debug or trace guest severity override.

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
tools/smoke-check.sh --bottle Steam
```

Hard failures mean the DLL or log file is missing. Warnings mean a required
smoke marker was not observed. Optional INFO markers are expected to be absent
unless debug/trace logging is enabled or the relevant UI path was exercised.
Common warning causes:

- Steam did not load the local `hid.dll`.
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

Older smoke runs may have left `crosspuck-driver-env.reg` or `CROSSPUCK_*`
values under `HKCU\Environment`. They are no longer part of the runtime
configuration path and can be removed with `regedit` if you want a clean bottle.
