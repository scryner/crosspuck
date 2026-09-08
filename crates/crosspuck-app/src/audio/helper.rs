//! Private child process: isolates HAL calls and permission waits from the UI/HID.
#[cfg(target_os = "macos")]
fn main() {
    unsafe extern "C" {
        fn crosspuck_audio_main(list_only: bool) -> i32;
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    let list_only = args.as_slice() == ["--list"];
    if !args.is_empty() && !list_only {
        std::process::exit(2);
    }
    // SAFETY: called once on the helper's main thread; owns its HAL objects,
    // signal handlers and parent-lifetime pipe until process exit.
    std::process::exit(unsafe { crosspuck_audio_main(list_only) });
}

#[cfg(not(target_os = "macos"))]
fn main() {
    std::process::exit(1);
}
