# CrossOver Guest Driver Smoke Test

This procedure verifies the production guest-side `hid.dll` in a CrossOver
Steam bottle. It is intentionally semi-automated: scripts handle file placement
and log checks, while Steam UI confirmation remains manual.

## Scope

The smoke test checks:

- Steam loads CrossPuck's `hid.dll` from the Steam application directory.
- The DLL installs hooks and connects to the macOS host bridge.
- SetupAPI/HID discovery reaches the virtual Steam Controller Puck profiles.
- `ReadFile`, `HidD_GetInputReport`, feature/output/write paths produce trace
  markers when exercised.
- Host disconnect and reconnect do not crash the Steam process.

The test does not prove long-session stability. Keep the 5 minute idle and
reconnect test as a separate pass before calling the driver release-ready.

## Prerequisites

- A CrossOver bottle with Steam installed.
- The macOS CrossPuck host app built and able to see the controller.
- A Windows/MSVC-built driver DLL:

```sh
cargo build -p crosspuck-driver --release --target x86_64-pc-windows-msvc
```

The expected output is:

```text
target/x86_64-pc-windows-msvc/release/hid.dll
```

On macOS without MSVC `link.exe`, use a Windows/MSVC machine or CI artifact to
produce this DLL.

## Install Into The Bottle

Install next to `Steam.exe`, not into `drive_c/windows/system32`.

```sh
tools/crossover/install-driver.sh --bottle Steam
```

Optional flags:

```sh
tools/crossover/install-driver.sh \
  --bottle Steam \
  --driver target/x86_64-pc-windows-msvc/release/hid.dll \
  --trace 1 \
  --required 1
```

The script:

- copies `hid.dll` into the detected Steam directory,
- backs up an existing local `hid.dll` under `crosspuck-backups/`,
- creates `crosspuck-driver-env.reg` in the bottle root,
- initializes a log file under the bottle user's `Temp` directory.

## Import Environment Variables

Import the generated registry file into the same bottle:

```text
<Bottle>/crosspuck-driver-env.reg
```

The registry file sets:

```text
CROSSPUCK_HOST_BRIDGE=1
CROSSPUCK_HOST_BRIDGE_REQUIRED=1
CROSSPUCK_TRACE_REPORTS=1
CROSSPUCK_LOG_FILE=C:\users\<user>\Temp\crosspuck-driver.log
```

One practical CrossOver path:

1. Open CrossOver.
2. Select the Steam bottle.
3. Use Run Command.
4. Run `regedit`.
5. Import `crosspuck-driver-env.reg`.
6. Quit Steam fully if it was already running.

If CrossOver does not pick up `HKCU\Environment` immediately, restart the
bottle or CrossOver before launching Steam.

## Run The Smoke

Start log watching first:

```sh
tail -f "$HOME/Library/Application Support/CrossOver/Bottles/Steam/drive_c/users/crossover/Temp/crosspuck-driver.log"
```

Start the macOS host app and confirm it sees the controller. Then start Steam
from the CrossOver Steam bottle.

Expected early log markers:

```text
[crosspuck] hook install ok
[crosspuck] crosspuck-driver attached host_bridge=true required=true trace=true
[crosspuck] startup bridge connect ok identity=Live profiles=5 open_handles=0
```

If the host app is not running yet, this marker is acceptable at first:

```text
[crosspuck] startup bridge connect failed: ...
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

## Success Criteria

Minimum pass:

- The driver log contains `crosspuck-driver attached`.
- The driver log contains `hook install ok`.
- The bridge eventually logs `startup bridge connect ok` or later trace proves
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
