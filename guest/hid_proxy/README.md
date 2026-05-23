# crosspuck-hid-proxy

Guest-side `hid.dll` proxy PoC for CrossOver.

This DLL claims the Steam Controller HID device path (`VID_28DE&PID_1304` by default), returns a virtual handle, and feeds the embedded `captures/a_button_taps.jsonl` packets through `ReadFile` / `HidD_GetInputReport`. Replay starts after 60 seconds unless overridden.

## Build

```powershell
cargo build --release --target x86_64-pc-windows-msvc
```

The DLL is emitted as:

```text
target\x86_64-pc-windows-msvc\release\hid.dll
```

## Run

Place `hid.dll` where the target process will load it before the system `hid.dll`.

```powershell
set CROSSPUCK_REPLAY_DELAY_MS=60000
```

Optional matching knobs:

```powershell
set CROSSPUCK_CLAIM_PATH_SUBSTR=vid_28de
set CROSSPUCK_CLAIM_ALL_HID=1
```

`CROSSPUCK_CLAIM_ALL_HID=1` is intentionally broad; use it only when a narrow path substring does not catch the target open.
