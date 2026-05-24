use crosspuck_core::guest_driver::{GuestDriverRuntime, RuntimeConfig};
use std::sync::OnceLock;

static RUNTIME: OnceLock<GuestDriverRuntime> = OnceLock::new();

pub fn init_runtime(config: RuntimeConfig) -> bool {
    RUNTIME.set(GuestDriverRuntime::new(config)).is_ok()
}

pub fn runtime() -> Option<&'static GuestDriverRuntime> {
    RUNTIME.get()
}
