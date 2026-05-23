# macOS Steam HID Reference Probe

This probe is a host-side reference tracer for the native macOS Steam client.
It interposes IOKit HID report calls and logs the real feature/output report
traffic plus input report callbacks for the Valve puck (`VID=0x28DE`,
`PID=0x1304` by default).

Build:

```sh
tools/macos_hid_probe/build.sh
```

Run native Steam with the probe:

```sh
tools/macos_hid_probe/launch_steam_with_probe.sh
```

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
STEAM_OSX=/Applications/Steam.app/Contents/MacOS/steam_osx
```

The key sequence to compare against the CrossOver guest is usually:

```text
SET type=feature report_id=...
GET request type=feature report_id=...
GET result type=feature report_id=... bytes=...
REGISTER input_report_callback ...
INPUT callback type=input report_id=... bytes=...
```

For the controller-recognition failure, capture from native Steam startup
through the point where the UI shows the Steam Controller as connected. The
important comparison points are native `02 B4`, `01 83`, `01 F2`, `01 AE`, and
the first input reports that lead to `WIRELESS SYSTEM DEBUG` / `Got bond` in
Steam's controller log.
