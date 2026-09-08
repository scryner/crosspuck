fn main() {
    println!("cargo:rerun-if-changed=src/audio/router.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // Only the helper references this library. The menu app retains its own
    // deployment target and checks availability before launching the helper.
    cc::Build::new()
        .file("src/audio/router.m")
        .flag("-fobjc-arc")
        .flag("-std=gnu11")
        .flag("-mmacosx-version-min=14.2")
        .warnings_into_errors(true)
        .compile("crosspuck_audio");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    println!("cargo:rustc-link-arg-bin=CrossPuckAudio=-mmacosx-version-min=14.2");
}
