# CrossOver audio routing PoC

Standalone macOS experiment for capturing CrossOver process audio and playing it
through a Core Audio output device. This does not change the CrossPuck app,
controller driver, Wine engine, bottle configuration, or system default output.

## Build and run

Requires Xcode command-line tools and macOS 14.2 or later. Only macOS 26.6.2 has
been exercised so far.

From the repository root:

```sh
sh tools/audio-route-poc/build.sh
poc_app="$PWD/target/audio-route-poc/CrossPuckAudioPoC.app"
poc_results="$PWD/target/audio-route-poc/results"

# Inventory only: CrossOver audio clients and the current default output.
"$poc_app/Contents/MacOS/CrossPuckAudioPoC" --list

# Capture 15 seconds without muting the game's normal output.
open -n -W "$poc_app" --args --capture --seconds 15 \
  --wav "$poc_results/capture.wav" --log "$poc_results/capture.jsonl"

# Route all currently active CrossOver audio clients for 60 seconds.
# A deliberate ten-second silence checks that original output is muted.
open -n -W "$poc_app" --args --route --seconds 60 \
  --gate-after 20 --gate-for 10 --log "$poc_results/route.jsonl"

# Follow macOS default-output changes for up to five minutes.
open -n -W "$poc_app" --args --follow-default --seconds 300 \
  --log "$poc_results/follow.jsonl"
```

Respond to the macOS system audio recording prompt before the Core Audio call
times out. After a timeout, approve the permission and restart the PoC. Launching
via `open` attributes permission to the PoC app; launching the binary directly
can attribute it to the terminal/parent app. Permission for one does not imply
permission for the other. Rebuilding an ad-hoc signed app may require approval
again. Logs contain a `cleanup.result` field; `open -W` alone does not report the
child's success or failure reliably.

Optional arguments:

- `--pid PID`: capture one validated CrossOver audio process.
- `--output-id ID`: use a current Core Audio device ID instead of the default.
  IDs can change; refresh the inventory before reusing them.
- `--seconds N`: run for 1–300 seconds, excluding device setup/permission waits.
  Follow mode accepts up to 900 seconds and uses one deadline across rebuilds,
  including setup time.
- `--wav PATH`: save local Float32 stereo WAV samples after stopping audio.
- `--log PATH`: write JSON lines, including input meters and cleanup status.
- `--follow-default`: route and rebuild on default-output, output-disconnect,
  or output sample-rate changes. Incompatible with `--output-id` and `--wav`.
- SIGUSR1: deliberately gate playback for `--gate-for` seconds (default 10).
  SIGINT/SIGTERM: stop the PoC and release the original-output mute.

## What it does

1. Enumerate Core Audio process objects, require the CrossOver Wine bundle ID
   prefix, and resolve `WINEPREFIX` to a bottle containing `cxbottle.conf`.
   Inspect only argv and the bottle environment entry; never log other
   environment values. Handle NUL padding in Darwin's process argument data.
2. Select all clients currently running output, across bottle paths, unless
   `--pid` narrows that set. Steam and the controller's selected bottle are not
   special cases. The PoC itself and native applications are excluded.
3. Create a private stereo process tap and private aggregate containing the
   chosen output device. Use that device as the main clock and enable tap drift
   compensation. Capture keeps originals unmuted. Routing uses
   `CATapMutedWhenTapped` and forwards samples in the same IO callback.
4. Map stereo to the output device's preferred left/right channels, including
   hardware that exposes more than two channels. Other output channels remain
   zero. A five-millisecond gain ramp limits abrupt start/gate transitions.
5. Stop the IOProc and destroy the aggregate and tap on timeout or SIGINT/SIGTERM.
   Normal cleanup releases the tap's original-output mute. Confirm audibly.

The audio callback uses preallocated storage and lock-free counters. It performs
no file writes, logging, allocation, or explicit locks. A requested WAV is held
in memory and written after the callback has stopped.

In follow mode, only CrossOver clients whose original output differs from the
macOS default are tapped. Clients already using the default keep their native
path. When all clients match, the PoC watches for changes without any tap or
aggregate (`passthrough` in the log). This preserves native volume when returning
to the game's original device. CrossOver clients and their output devices are
re-enumerated once per second, including while waiting in passthrough.

A Core Audio default-output listener records change generations;
the control loop also checks the current device, availability, and sample rate
every 200 ms. The old IOProc, aggregate, and tap are destroyed before rebuilding
against the latest default device. A 250 ms quiet period coalesces changes.
The original game output is unmuted during teardown/rebuild, so this first
version may briefly play through the original speaker during a transition.

Each new route re-enumerates CrossOver audio processes, checks aggregate
readiness and actual IO formats, and selects the new hardware's preferred
stereo pair. Physical input streams are explicitly disabled with
`kAudioDevicePropertyIOProcStreamUsage` and checked by readback before starting
audio; the callback rejects any unexpectedly present physical input buffer.
An invalid buffer layout or two seconds without frame progress triggers cleanup.
Failed setup/playback attempts use bounded backoff and stop after three failures.
Permission and HAL setup calls remain synchronous in this standalone tool.

## Live observations: 2026-09-08

Host: macOS 26.6.2; CrossOver Preview; `OnimushaWotS.exe` already playing in the
Steam bottle. The game stayed in the same process during testing.

- Process discovery correctly identified the game separately from inactive
  Steam and `steamwebhelper` audio clients. The game used the Studio Display XDR
  speakers, also the macOS default output.
- Capture succeeded: **720,896 stereo frames at 48,000 Hz**, a **15.019-second**
  local Float32 WAV. Nonzero input was measured, with **0 bad buffers** and
  successful Core Audio cleanup. `afinfo` independently verified the WAV format
  and duration.
- The display's output is **8-channel Float32**, while the tap is stereo. The
  first routing attempt correctly stopped on that mismatch. The PoC now uses
  the hardware's preferred stereo channels **1 and 2**.
- A subsequent **60-second** routing run handled **2,881,536 frames**, including
  a deliberately gated interval of **142,848 frames (2.976 seconds)**, with
  **0 bad buffers** and successful cleanup. The maximum measured callback work
  was **0.019 ms**. These counters establish data flow; listening is needed to
  establish audible playback and original-output suppression.
- After that run, the game still had its original PID and active output to the
  same speaker. The macOS default output UID was unchanged.
- Repeated the run with a **10-second gate** at the user's request. The measured
  gated interval was **480,256 frames (10.005 seconds)**. The user confirmed
  normal playback, complete silence during the gate, resumed playback, and
  normal sound after the PoC ended. This validates the capture → original mute
  → playback → original-output restoration path for this game and speaker.
- Initial permission waits caused `MACH_RCV_TIMED_OUT` (`0x10004003`). Retrying
  after approval worked without restarting the game or the system audio daemon.
- The callback's input/output HAL timestamp gap is approximately **23.17 ms**
  with a **512-frame** device buffer. This is a timestamp observation, not a
  measured acoustic end-to-end latency or a claim about added game latency.

### Stage 3 findings

- Reproduced the original problem before the PoC: macOS default output was
  AirPods Pro, while `OnimushaWotS.exe` remained on the Studio Display speakers.
- Starting the follower rerouted the running game's audio to AirPods Pro;
  the user confirmed audio only from AirPods. Both devices used 48 kHz, so this
  test does not establish differing-rate conversion behavior.
- AirPods Pro exposed a separate 48 kHz output device and 24 kHz microphone
  device. Only the output device entered the aggregate. The tap delivered
  stereo Float32, with 512-frame callbacks and a HAL timestamp gap around 44 ms.
- The first follower rebuilt successfully after AirPods disconnected and
  rendered through the speaker, with no bad buffers. However, the user reported
  low speaker volume; the volume returned to normal when the PoC ended.
- The updated follower uses the native path whenever the game's original output
  already matches the default. It no longer unnecessarily captures/replays the
  speaker return path. Multichannel tap level behavior is still a production
  validation concern for paths that genuinely require rerouting; no guessed
  gain compensation is applied.
- A diagnostic attempt to set the default output through Core Audio briefly
  selected the speaker, then macOS's in-ear routing policy reselected AirPods.
  The live test therefore uses actual AirPods case removal/insertion. The PoC
  itself never sets the macOS default output.
- With the selective follower running, changing the default to the Mac Studio
  built-in speaker automatically began routing; **442,880 frames** were handled
  with **0 bad buffers**. Returning to Studio Display destroyed both temporary
  audio objects successfully and returned to `passthrough` in the same PoC
  process. This test used the real Core Audio default-output setter in a
  separate temporary diagnostic executable.
- **Selective AirPods round trip passed:** from native speaker passthrough,
  wearing AirPods automatically started rerouting. A transient default-output
  bounce during Bluetooth startup was cleaned up and retried. The stable AirPods
  segment forwarded **1,367,040 frames (28.48 seconds)** with **0 bad buffers**.
  Returning AirPods to their case released the tap and aggregate and restored
  native speaker passthrough. The user confirmed **both AirPods playback and
  return to the speaker's original volume**. AirPods obtained a different
  AudioDeviceID on reconnection; the follower rediscovered it correctly.
- The selective test used a single PoC process. Onimusha remained PID 19678
  throughout those tests. The PoC was then stopped with SIGTERM, removed its
  listener, and exited successfully; the game continued through the Studio
  Display speakers.

### AirPods Max validation

Repeated the test with a newly launched `OnimushaWotS.exe` (PID 85138) using the
same selective-follower binary. The initial game and macOS default output were
the Studio Display speakers. The follower began in native passthrough and
automatically created the route when AirPods Max became the default output.

- AirPods Max exposed a separate **48 kHz output** and **24 kHz microphone**.
  The aggregate included only the output device.
- The route used **48 kHz Float32 stereo**, the preferred channels **1 and 2**,
  and **512-frame** callbacks. The user confirmed that audio was heard only
  from AirPods Max, with normal volume and no audible interruption or distortion.
- Disconnecting Max returned to native Studio Display passthrough. Reconnecting
  Max automatically recreated the route in the same PoC process. The user
  confirmed both **original speaker volume** and **normal Max reconnection**.
- The two Max segments forwarded **7,545,344 frames (157.195 seconds)**, with
  **0 bad buffers** and successful audio-object cleanup after each segment.
  No PoC code changes were needed for Max.
- After SIGTERM, the follower removed its listener and exited successfully.
  The same game PID continued on its original speaker device; the macOS default
  remained AirPods Max. This is restoration of the original game routing when
  the helper is disabled, not continued forwarding after exit.
  The user also confirmed normal speaker playback at its original volume after
  the PoC exited.
- Logs for this run are `airpods-max-follow.jsonl` and
  `airpods-max-devices.jsonl` in the local results directory; the checked summary
  is `airpods-max-summary.json`.

Generated audio and JSON logs are in ignored `target/audio-route-poc/results/`;
they are local test artifacts, not repository source files.

## Scope and remaining validation

- Persistent enable/disable settings and the menu item are absent.
- Follow mode refreshes targets once per second and rebuilds when the target set
  changes. Late process detection, process exit/restart races, and simultaneous
  audio from multiple bottles need additional live tests. This session has only
  one audible bottle. Fixed capture/route modes still use a startup snapshot.
- Duplex hardware input-disabling is implemented defensively but still needs
  live validation on a device exposing both directions under one device ID.
  The tested AirPods Pro exposes separate input and output device IDs, so its
  microphone is never included in the private aggregate.
- Input must be Float32 stereo. Output must be Float32 at the same sample rate,
  expose a preferred stereo pair, and deliver the same frame count. Other
  sample rates/layouts require conversion or a different playback path.
- No explicit ring buffer/resampler or long-duration drift, load, sleep/wake,
  or hard-crash recovery validation is included yet. Gate-based
  listening confirms original-output suppression; timestamps alone cannot
  establish audible quality or latency.
- Production integration should isolate blocking permission/setup calls from
  the app UI, strengthen failure supervision around synchronous HAL operations,
  and validate volume/channel conversion across source and destination devices.

API references:
[Apple Core Audio taps sample](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps),
[CATapDescription](https://developer.apple.com/documentation/coreaudio/catapdescription).
