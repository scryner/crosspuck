# Automatic CrossOver audio output

CrossPuck follows the macOS default output for all active CrossOver audio
clients. This service starts independently of controller discovery and the
selected Steam bottle. `FollowAudioOutput` in UserDefaults defaults to true
when absent; the menu persists explicit enable and disable choices.

## Engine and ownership

`crosspuck-app/src/audio/mod.rs` supervises the bundled `CrossPuckAudio`
executable. The Objective-C Core Audio engine is in `audio/router.m`; Cargo
builds it with Clang through `cc`. The helper requires macOS 14.2. The menu app
checks OS availability before launching it and does not reference the newer
tap symbols, preserving the existing controller deployment target.

The helper discovers Core Audio process objects with a CrossOver bundle ID,
then verifies their `WINEPREFIX` contains `cxbottle.conf`. It reads only the
command name and WINEPREFIX from process arguments. It does not select clients
by parent PID, controller bottle, game filename, or a fixed device ID.

For an active process whose output differs from the default, the engine creates
a private stereo process tap using `CATapMutedWhenTapped`, followed by a private
aggregate containing the destination and tap. Physical input streams are
disabled and read back before starting IO. The callback copies stereo Float32
samples to the destination's preferred stereo channels, zeros other channels,
and ramps in over 5 ms. It allocates no memory, takes no locks, performs no
logging and makes no HAL configuration calls. No PCM is sent to the menu app.

HAL tap drift compensation adapts to the aggregate clock. The engine validates
the actual aggregate input/output formats and matching frame counts instead of
assuming that the source tap's nominal rate equals the device rate. Unsupported
formats fail before starting where detectable; callback failures release the
route. There is no independent software resampler or arbitrary gain correction.

The control loop polls output health and listens for default-output changes.
It checks active clients and stream formats once per second. A changed output,
client set, rate or stream layout tears down the route and rebuilds it after a
250 ms quiet interval. Digital silence counts as valid frame progress. Missing
frames for two seconds or malformed output buffers cause cleanup and retry.

Control waits use a registered run-loop source: output notifications and owner
exit wake it immediately, with health checks at 200 ms while routing and client
scans at one second while idle. There is no fixed 10 ms sleep loop. The parent
supervisor parks until a helper event, setting change or deadline; when disabled
it has no polling timer. The owner watchdog blocks on stdin and a signal-safe
shutdown pipe. Known-size HAL properties use stack storage, and the recurring
client scan skips inactive/native-output clients before inspecting their process
environment; full device metadata is collected only for `--list`.

When all clients already use the default, no tap is created. This is essential
for returning to Studio Display at native volume: routing a stereo mixdown
back onto that same multichannel device reduced volume in the initial PoC.
The implementation does not apply a guessed compensation factor.

The parent owns a pipe to the helper. Disable and quit close that pipe;
unexpected parent exit closes it automatically. A helper watchdog then requests
cleanup and exits after three seconds if a HAL call is stuck. The supervisor
allows four seconds before killing/reaping the child. A per-user file lock
prevents two copies from routing simultaneously. Cleanup failures cause a full
helper restart, avoiding reuse of potentially invalid callback state.

The last public audio-service handle stops and joins its worker. The worker
owns separate shared state, so it cannot keep the service owner alive itself.
Menu objects remain scoped across `app.run()` rather than being explicitly
leaked. Native retry/transition iterations have their own autorelease pool,
and both startup failures and shutdown release the watcher, pipe, run-loop
source and lease.

State and heartbeat JSON lines carry control information only. The parent
logs state changes at info level and detailed setup/cleanup at debug level.
Ordinary retries back off to 30 seconds. A connecting helper gets 90 seconds
for the initial macOS permission exchange; an active helper must continue
reporting within 10 seconds. HID and the menu stay responsive during these
waits. Granting access after a HAL timeout may require a helper restart, which
the supervisor performs automatically.

## Build and automated checks

```sh
sh tools/build-app.sh release
sh tools/test-audio-engine.sh
cargo test --workspace
cargo check --workspace
cargo fmt --check
```

The native tests run under AddressSanitizer and UndefinedBehaviorSanitizer.
They cover process argument padding/truncation, excluding unrelated environment
values, multiple bottle target selection, native-output bypass, channel mapping,
digital silence, non-finite samples, clipping, incompatible buffer sizes and
disabled physical inputs. Native tests also check sleeping with an otherwise
empty run loop, notifications before/during a wait, parent EOF, signal wakeup
and joining an idle watchdog. Rust tests cover control-state parsing, bounded
retry delays, cooperative child shutdown, forced termination of a stuck child,
last-handle cleanup and idempotent shutdown of a parked worker.
Existing controller/transport tests use local TCP sockets and therefore
need an environment that permits loopback connections.

`tools/build-app.sh` bundles and ad-hoc signs the helper and app. For release,
`tools/build-dmg.sh --app-sign-identity ...` signs the nested helper first with
the same Developer ID. Developer builds can need renewed macOS permissions
after the executable/signature changes; release signing should retain the
same identity. Never reset the user's TCC database as part of building.

Read-only diagnostics, without creating a tap:

```sh
target/release/CrossPuck.app/Contents/MacOS/CrossPuckAudio --list
```

## Hardware validation

The preceding PoC was verified with Onimusha: Way of the Sword
(`OnimushaWotS.exe`), Studio Display speakers, AirPods Pro and AirPods Max:

- Attach after the game starts, capture and reproduce live audio.
- A 10-second gate completely silences the game and resumes playback.
- Speaker to headphones follows the default, with audio only in headphones.
- Headphone disconnect returns to speakers at the original volume.
- Reconnecting headphones follows newly discovered device IDs.
- Stopping the router restores original game playback and volume.

The tested routes used stereo Float32 at 48 kHz. The Max round trip processed
7,545,344 frames with zero malformed buffers; the largest measured PoC callback
was about 0.018 ms. These figures describe callback work, not end-to-end latency.

Additional live coverage is still needed for simultaneous games in multiple
bottles, 44.1/48 kHz transitions, HDMI/USB DACs, duplex output devices, sleep/wake,
and long sessions. Multiple-bottle selection and physical-input exclusion have
automated coverage; that does not replace those hardware tests. Unsupported
layouts preserve the native route while retrying.

Production-app acceptance also checks the actual menu toggle, saved disabled
state across restart, default-on late attachment, and recovery after helper or
parent termination. Local run logs are kept under ignored `target/` directories.

On 2026-09-08 the production bundle passed late attachment to the running
Onimusha process, menu disable/enable and persistence of a disabled preference
across restart. The user confirmed normal AirPods Max playback with the final
bundle. Killing its helper resulted in a replacement in about 1 second and
resumed routing in about 1.4 seconds. Killing the parent caused its helper to
exit in about 0.1 seconds; relaunching restored routing. A second helper refused
to acquire the routing lease (exit 11).

The final production bundle also passed the user-verified AirPods Max → Studio
Display → AirPods Max round trip, including original speaker volume. Logs show
complete route cleanup with zero malformed buffers, native passthrough on the
speaker, and reconnection using the Max's newly assigned device ID. Transient
Bluetooth default changes during reconnection also caused clean rebuilds.

All 133 Rust tests passed, as did the native sanitizer tests, workspace check,
Windows GNU driver check, formatting, shell syntax and bundle signature checks.
Strict Clippy is blocked by existing warnings in controller/installer code
(`nonminimal_bool`, `ptr_arg`, `map_entry`, `too_many_arguments`,
`large_enum_variant`). A separate export of the original HEAD reproduced the
host warnings. Allowing those existing lint categories produced clean host and
Windows GNU runs; controller sources were left unchanged.

## Memory and CPU review (2026-09-08)

The review corrected temporary-object retention across retry iterations,
100 ms polling in the Rust supervisor, a potentially immediate-return native
run loop, and periodic wakeups in the owner watchdog. It also removed the
explicit menu-owner leak and made dropping the last audio handle terminate
its worker. Retry backoff now resets only after a healthy waiting/routing
period, rather than after a long unsuccessful permission wait.

An accelerated test repeated retry-state generation 10,000 times at `-O2`.
Before the fix, resident memory grew by 44.6 MiB within the outer autorelease
pool; with per-iteration draining and scalar property reads it did not grow.
This is a stress test of the retry path, not the growth rate of a normal game
session.

Local helper measurements used `proc_pid_rusage`, excluding the first five
seconds. CPU percentages are relative to one CPU core. Both baseline and
updated runs used the same running Steam instance and output devices:

| Condition | Before | After |
| --- | ---: | ---: |
| Waiting, 30 seconds | 0.018% CPU | 0.003% CPU |
| Onimusha routed from its original AirPods Max output to Studio Display | 0.014% CPU (30 s) | 0.009% CPU (60 s) |

The updated live route processed 2,923,008 frames with zero bad buffers and
successful cleanup. Post-warmup RSS changed by 32 KiB over that measurement.
`leaks` reported zero leaked bytes in the helper in both idle and active tests.
These short measurements establish no sustained CPU hog in the tested paths;
they do not replace a multi-hour soak or additional hardware coverage.

The updated menu app passed disable (0.17 s), enable (0.68 s), helper-crash
recovery (1.46 s), default-output round trips, parent-crash cleanup (0.13 s)
and normal Quit. Sampling showed the disabled supervisor blocked in
`std::thread::park`. The macOS default output was restored to AirPods Max and
all test processes were stopped.

The menu-app leak report contains three `NSXPCConnection` root cycles. With
allocation-stack logging these originate in Apple's AppIntents/LinkServices
`LNProcessInstanceRegistryClient makeXPCConnection`. A minimal AppKit program
containing only NSApplication initialization and its run loop reproduces the
same three roots, without any CrossPuck/audio code. These system-framework
reports are distinct from the corrected application ownership and retry-pool
issues; the full menu-app report is therefore not claimed to be zero.

All 135 Rust tests passed; native AddressSanitizer/UndefinedBehaviorSanitizer
regressions and the Clang static analyzer passed. Clippy passed with only the
previously documented existing lint categories allowed. After the menu-owner
change, the app tests, Clippy, release build and live menu lifecycle checks
were repeated. Logs and comparison fixtures are in ignored
`target/audio-review/`. The review did not rebuild/notarize the distribution
DMG or copy an app to /Applications; existing DMGs predate these fixes.
