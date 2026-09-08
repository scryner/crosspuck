# Late-start discovery PoC

This experiment tests whether Steam already running in CrossOver can discover
CrossPuck after the host starts. It uses the installed host and guest driver;
the PoC did not change production Rust code, DLLs, or Wine registry settings.
The production implementation that followed this experiment is described in
[`../../crates/crosspuck-driver/README.md`](../../crates/crosspuck-driver/README.md).

`notify.c` finds exactly one `steam.exe` in the current bottle, lists its HID/SDL
modules and windows, and optionally sends one `WM_DEVICECHANGE / DBT_DEVICEARRIVAL`
message to its `SDL_HIDAPI_DEVICE_DETECTION` window. This is a diagnostic rescan
trigger, not the implementation of automatic reconnect.

The SDL discovery window is message-only, so ordinary `EnumWindows` or a
top-level-window broadcast is insufficient. The helper enumerates message-only
windows with `FindWindowExA(HWND_MESSAGE, ...)` and filters by both Steam PID and
the exact SDL class. It uses `SendMessageTimeoutA` with a sized device-interface
payload instead of posting an asynchronous raw pointer. It does not inject code
or alter another process's memory. The notification contains an empty device
name and is intended only for this SDL receiver, which treats it as a rescan
hint; it is not a general-purpose synthetic PnP device announcement.

Reference: [SDL HID discovery implementation](https://github.com/libsdl-org/SDL/blob/main/src/hidapi/SDL_hidapi.c).

## Build and run

From the repository root, with MinGW installed:

```sh
mkdir -p target/late-start-poc
x86_64-w64-mingw32-gcc -std=c11 -Wall -Wextra -Werror -O2 \
  tools/late-start-poc/notify.c -o target/late-start-poc/notify.exe -luser32
```

Use the CrossOver installation that owns the running bottle. The tested instance
was CrossOver Preview and its `Steam` bottle:

```sh
"/Applications/CrossOver Preview.app/Contents/SharedSupport/CrossOver/bin/wine" \
  --bottle Steam "$PWD/target/late-start-poc/notify.exe" --list
```

After CrossPuck is listening and the Puck is available:

```sh
"/Applications/CrossOver Preview.app/Contents/SharedSupport/CrossOver/bin/wine" \
  --bottle Steam "$PWD/target/late-start-poc/notify.exe" --arrival
```

Exit status is nonzero for ambiguous/missing Steam, missing SDL receiver, or
notification delivery failure. Delivery success alone does **not** establish
controller discovery or working input. Check fresh driver/Steam log entries and
the controller UI. Do not repeatedly send notifications as a polling mechanism:
the original PoC-tested driver deliberately retained augmented SDL enumeration
allocations. The subsequent production implementation fixes this ownership.

## Observed result — 2026-09-08, Asia/Seoul

Environment: branch `fix/allow-after-crossover-executed`, base commit `45bb34c`.
The running Steam had host PID `51529` / Wine PID `268`; these stayed unchanged
during the initial late-start test. The installed guest DLL SHA-256 was
`23aa5f7d39c08defcfd8c011c9ecada0a49b4f48e1f4b3afb6beeab7854c37f1`, identical to
the DLL embedded in `/Applications/CrossPuck.app`. This identifies the tested
installed binary; it does not imply a rebuild of the current checkout.

| Phase | Observation |
| --- | --- |
| Steam running, CrossPuck stopped | Driver loaded; `SDL_hid_enumerate` logged connection refused (`10061`); Steam's controller list was empty. |
| Receiver inventory | Exactly one `SDL_HIDAPI_DEVICE_DETECTION` message-only window belonged to Steam. Both local CrossPuck HID.DLL and SDL3.dll were loaded. |
| 18:32:38 — host started with guest debug override | CrossPuck listened on 127.0.0.1:28473 and :28474. A 20-second observation added zero bytes to both driver and controller logs. Connections were still absent when checked before the notification several minutes later. |
| 18:36:25 — one arrival notification | Delivery returned `sent=1 result=1 error=0`. Same Steam process connected via `SDL_hid_enumerate`, advertised five profiles, and opened all five virtual paths. |
| 18:36:26 | Steam recorded `Data is flowing` and `wireless_established`. |
| 18:36:27 | Steam recorded `Controller Connected to Dongle`. |
| UI check | Existing settings page updated to show Steam Controller and Steam Controller Puck without restarting Steam or reopening settings. The input-test page opened successfully. |
| Physical input check | User confirmed that physical input was reflected normally in Steam's input-test screen. |

Raw log slices for this local experiment are in the ignored directory
`target/late-start-poc/`: `baseline.json`, `host-only-*`, and `after-arrival-*`.
They are local evidence and are not required to run the helper.

## Interpretation and limits

- Initial late connection, HID access, virtual enumeration, and Steam device
  initialization work with the existing binaries once SDL is prompted to rescan.
- This reproduction strongly supports the missing discovery trigger as the
  immediate cause. Native handle takeover was not an obstacle in this run:
  `SDL_hid_enumerate` reported an empty original list before synthetic entries.
- A targeted Windows notification works in the tested Steam/SDL/CrossOver
  combination. A new hook of `SDL_hid_device_change_count` is therefore not the
  only feasible way to trigger discovery. That hook itself has not been tested.
- This helper does not prove autonomous retries, host-restart recovery,
  unavailable/late physical HID handling, multiple bottles, or long-term resource
  stability. Those need separate implementation and regression tests.
- Physical input was confirmed by the user. Steam's Ping action was invoked once;
  physical feedback confirmation is pending.

## Implementation follow-up

The production implementation uses a guest-owned connection worker and targeted
SDL notification. It waits for identity and input attachment, serializes
connection attempts, tracks connection generations, retries unavailable receivers,
and owns/frees augmented enumeration lists. The host opens its HID input reader
before acknowledging a usable session. The PoC helper is no longer needed for
normal startup or reconnect.
