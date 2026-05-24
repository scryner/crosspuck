# crosspuck-hid-proxy

Guest-side `hid.dll` proxy PoC for CrossOver.

This DLL claims the Steam Controller HID device path (`VID_28DE&PID_1304` by default), returns a virtual handle, and feeds the embedded `captures/a_button_taps.jsonl` packets through `ReadFile` / `HidD_GetInputReport`. Replay starts after 60 seconds unless overridden.

When the macOS host app is running, the proxy can use the real host transport instead of only replay/synthetic responses:

```powershell
set CROSSPUCK_HOST_BRIDGE=1
```

With this enabled, virtual `ReadFile`/`HidD_GetFeature`/`HidD_SetFeature`/`HidD_SetOutputReport`/`SDL_hid_*` paths first call the shared `crosspuck-core` guest transport runtime. If the bridge is not connected, the proxy retries lazily while keeping the existing replay/synthetic fallback path available. When connected, HID attributes and SDL enumeration use the host-provided identity.

## Build

```powershell
cargo build --release --target x86_64-pc-windows-gnu
```

The DLL is emitted as:

```text
target\x86_64-pc-windows-gnu\release\hid.dll
```

## Run

Place `hid.dll` where the target process will load it before the system `hid.dll`.

```powershell
set CROSSPUCK_REPLAY_DELAY_MS=60000
set CROSSPUCK_HOST_BRIDGE=1
```

Optional matching knobs:

```powershell
set CROSSPUCK_CLAIM_PATH_SUBSTR=vid_28de
set CROSSPUCK_CLAIM_ALL_HID=1
set CROSSPUCK_HOST_BRIDGE_CONNECT_TIMEOUT_MS=5000
set CROSSPUCK_HOST_BRIDGE_IO_TIMEOUT_MS=50
```

`CROSSPUCK_CLAIM_ALL_HID=1` is intentionally broad; use it only when a narrow path substring does not catch the target open.
