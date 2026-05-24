# crosspuck-driver

Production guest-side `hid.dll` crate for CrossPuck.

This crate owns the Windows DLL boundary: `DllMain`, hook installation, Win32 ABI buffers, handles, and error mapping. Protocol transport, host bridge runtime, virtual HID identity/profile calculations, and byte-preserving HID I/O routing live in `crosspuck-core::guest_driver`.

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
