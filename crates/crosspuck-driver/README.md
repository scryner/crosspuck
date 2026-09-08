# crosspuck-driver

Production guest-side `hid.dll` crate for CrossPuck.

This crate owns the Windows DLL boundary: `DllMain`, hook installation, Win32 ABI buffers, handles, and error mapping. Protocol transport, host bridge runtime, virtual HID identity/profile calculations, and byte-preserving HID I/O routing live in `crosspuck-core::guest_driver`.

In `steam.exe`, a process-lifetime worker retries host discovery independently of
Steam's HID calls. Once identity and input-channel attachment are ready, it sends
a HID device-change notification to SDL's message-only detection window in the
same process. Connection generations trigger arrival notifications; loss of a
connection triggers removal. Missing or unresponsive receivers are retried. This
allows CrossPuck to start or restart after Steam without reopening controller
settings. Lazy connection attempts remain available and share a serialized
connection lifecycle with the worker.

SDL enumeration results are copied into driver-owned lists, including native
entries, and released by the matching free hook. Repeated discovery therefore
does not leak synthetic lists or mix SDL and Rust allocation ownership.

Build the target DLL with:

```sh
cargo build -p crosspuck-driver --release --target x86_64-pc-windows-gnu
```

The output DLL path is:

```text
target/x86_64-pc-windows-gnu/release/hid.dll
```

`x86_64-pc-windows-msvc` is also supported for type checking and Windows-native
builds, but it requires MSVC `link.exe`. On macOS/CrossOver development
machines, the GNU target is the practical local release build target.

CrossOver smoke-test procedure and helper scripts are documented in:

- [`docs/crossover-smoke.md`](docs/crossover-smoke.md)
- [`docs/crossover-smoke-ko.md`](docs/crossover-smoke-ko.md)
