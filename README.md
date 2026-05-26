# CrossPuck

CrossPuck lets Steam running in a CrossOver bottle use a Steam Controller
connected to the macOS host.

The project is split into two production pieces:

- `crosspuck-app`: a macOS menu bar host app. It reads the physical Steam
  Controller/Puck HID devices, forwards input reports to the guest, and applies
  guest feedback such as rumbles/haptics back to the controller.
- `crosspuck-driver`: a guest-side `hid.dll` for the CrossOver Steam process.
  It exposes virtual HID profiles to Steam and bridges input/output traffic to
  the macOS host app.

Shared HID identity, transport, protocol, and guest runtime logic live in
`crosspuck-core`. `crosspuck-cli` contains development and diagnostic tools.

## Pre-requisites

- macOS with Rust installed.
- Steam Puck/Controller visible to the macOS host.
- Steam Puck/Controlelr paired with native app(on macOS or Windows).
- CrossOver with Steam installed in a bottle.
- Windows Rust target for the guest DLL:

```sh
rustup target add x86_64-pc-windows-gnu
```

## Build The Host App

Build a debug app bundle:

```sh
tools/crosspuck/build-app.sh debug
```

Build a release app bundle:

```sh
tools/crosspuck/build-app.sh release
```

The script prints the generated bundle path, for example:

```text
target/release/CrossPuck.app
```

Start the app before launching Steam in the CrossOver bottle.

## Build The Guest Driver

Build the production guest `hid.dll`:

```sh
cargo build -p crosspuck-driver --release --target x86_64-pc-windows-gnu
```

The output is:

```text
target/x86_64-pc-windows-gnu/release/hid.dll
```

## Install Into CrossOver

Install the driver next to `Steam.exe` in the target bottle:

```sh
tools/crosspuck/install-driver.sh --bottle Steam
```

Useful options:

```sh
tools/crosspuck/install-driver.sh \
  --bottle Steam \
  --driver target/x86_64-pc-windows-gnu/release/hid.dll \
  --no-build
```

The script copies `hid.dll`, backs up any existing local `hid.dll`, and
initializes `crosspuck-driver.log`. It does not write guest runtime
`CROSSPUCK_*` registry or environment settings; guest runtime settings come
from built-in defaults and host connection overrides.

Do not install this DLL into `drive_c/windows/system32`. It is designed to live
next to Steam and forward non-virtual HID calls to the real system HID DLL.

If CrossOver needs an explicit loader override for the app-local `hid.dll`, ask
the installer to write a loader-only registry file:

```sh
tools/crosspuck/install-driver.sh --bottle Steam --write-wine-override
```

That file only sets Wine's `hid=native,builtin` DLL override. It does not set
guest runtime options.

## Tools And Diagnostics

Tooling, logging options, smoke checks, development verification commands, and
the macOS HID reference probe are documented in `tools/README.md`.

## License

CrossPuck is licensed under the Apache License, Version 2.0. See
`LICENSE`.

Third-party dependency license notices are listed in
`THIRD-PARTY-NOTICES.md`.
