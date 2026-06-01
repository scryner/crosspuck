use super::log::{debug_line, error_line, info_line, set_log_level, set_session_trace_id};
use crosspuck_core::guest_driver::{
    GuestDriverRuntime, GuestDriverSnapshot, GuestLogLevel, RuntimeConfig, RuntimeIdentityState,
    VirtualHidProfileCatalog,
};
use std::sync::{Mutex, OnceLock};

static RUNTIME: OnceLock<GuestDriverRuntime> = OnceLock::new();
static BRIDGE_CONNECT_ALLOWED: OnceLock<bool> = OnceLock::new();
static LAST_CATALOG_LOG_STATE: OnceLock<Mutex<Option<CatalogLogState>>> = OnceLock::new();
static LAST_BRIDGE_CONNECT_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static LAST_HOST_LOG_LEVEL_OVERRIDE: OnceLock<Mutex<Option<GuestLogLevel>>> = OnceLock::new();

#[derive(Clone, Debug, Eq, PartialEq)]
struct CatalogLogState {
    identity_state: RuntimeIdentityState,
    bridge_connected: bool,
    advertised_profiles: usize,
}

impl From<&GuestDriverSnapshot> for CatalogLogState {
    fn from(snapshot: &GuestDriverSnapshot) -> Self {
        Self {
            identity_state: snapshot.identity_state,
            bridge_connected: snapshot.bridge_connected,
            advertised_profiles: snapshot.advertised_profiles,
        }
    }
}

pub fn init_runtime(config: RuntimeConfig, bridge_connect_allowed: bool) -> bool {
    let _ = BRIDGE_CONNECT_ALLOWED.set(bridge_connect_allowed);
    RUNTIME.set(GuestDriverRuntime::new(config)).is_ok()
}

pub fn runtime() -> Option<&'static GuestDriverRuntime> {
    RUNTIME.get()
}

pub fn catalog(reason: &'static str) -> Option<VirtualHidProfileCatalog> {
    let runtime = runtime()?;
    if !bridge_connect_allowed() {
        let catalog = runtime.catalog_if_connected();
        if catalog.is_none() {
            let snapshot = runtime.snapshot();
            debug_line(&format!(
                "[crosspuck] cached catalog unavailable reason={reason} bridge_connect_allowed=false bridge_connected={} identity={:?} profiles={}",
                snapshot.bridge_connected, snapshot.identity_state, snapshot.advertised_profiles
            ));
        }
        return catalog;
    }

    let before = runtime.snapshot();
    let catalog = match runtime.catalog_result() {
        Ok(catalog) => catalog,
        Err(error) => {
            log_bridge_connect_failed(runtime, reason, &error.to_string());
            return None;
        }
    };
    apply_connection_logging(runtime);
    let after = runtime.snapshot();
    log_catalog_state(reason, &before, &after, catalog.is_some());
    catalog
}

pub fn catalog_if_connected(reason: &'static str) -> Option<VirtualHidProfileCatalog> {
    let runtime = runtime()?;
    let catalog = runtime.catalog_if_connected();
    if catalog.is_some() {
        apply_connection_logging(runtime);
    }
    if catalog.is_none() {
        let snapshot = runtime.snapshot();
        debug_line(&format!(
            "[crosspuck] cached catalog unavailable reason={reason} bridge_connected={} identity={:?} profiles={}",
            snapshot.bridge_connected, snapshot.identity_state, snapshot.advertised_profiles
        ));
    }
    catalog
}

fn log_bridge_connect_failed(
    runtime: &GuestDriverRuntime,
    reason: &'static str,
    error_message: &str,
) {
    let Ok(mut last) = LAST_BRIDGE_CONNECT_ERROR
        .get_or_init(|| Mutex::new(None))
        .lock()
    else {
        return;
    };
    if last.as_deref() == Some(error_message) {
        return;
    }
    *last = Some(error_message.to_string());
    set_session_trace_id(runtime.session_trace_id());
    error_line(&format!(
        "[crosspuck] lazy bridge connect failed reason={reason}: {error_message}"
    ));
}

fn clear_bridge_connect_error() {
    if let Ok(mut last) = LAST_BRIDGE_CONNECT_ERROR
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *last = None;
    }
}

fn apply_connection_logging(runtime: &GuestDriverRuntime) {
    let snapshot = runtime.snapshot();
    if !snapshot.bridge_connected {
        set_session_trace_id(None);
        set_log_level(runtime.config().log_level);
        let _ = host_override_changed(None);
        return;
    }

    set_session_trace_id(snapshot.session_trace_id);
    let host_override = runtime.guest_log_level_override().map(GuestLogLevel::from);
    let effective_level = host_override.unwrap_or(runtime.config().log_level);
    set_log_level(effective_level);

    if host_override_changed(host_override) {
        if let Some(log_level) = host_override {
            info_line(&format!(
                "[crosspuck] host log level override applied level={}",
                log_level.as_str()
            ));
        } else {
            debug_line("[crosspuck] host log level override cleared");
        }
    }
}

fn host_override_changed(next: Option<GuestLogLevel>) -> bool {
    let Ok(mut guard) = LAST_HOST_LOG_LEVEL_OVERRIDE
        .get_or_init(|| Mutex::new(None))
        .lock()
    else {
        return true;
    };
    if *guard == next {
        return false;
    }
    *guard = next;
    true
}

fn bridge_connect_allowed() -> bool {
    BRIDGE_CONNECT_ALLOWED.get().copied().unwrap_or(true)
}

fn log_catalog_state(
    reason: &'static str,
    before: &GuestDriverSnapshot,
    after: &GuestDriverSnapshot,
    catalog_available: bool,
) {
    let state = CatalogLogState::from(after);
    let lock = LAST_CATALOG_LOG_STATE.get_or_init(|| Mutex::new(None));
    let Ok(mut last) = lock.lock() else {
        return;
    };
    let changed = last.as_ref() != Some(&state);
    if !(changed || catalog_available && !before.bridge_connected && after.bridge_connected) {
        return;
    }

    if after.bridge_connected {
        clear_bridge_connect_error();
        set_session_trace_id(after.session_trace_id);
        info_line(&format!(
            "[crosspuck] lazy bridge connect ok reason={reason} identity={:?} profiles={} open_handles={}",
            after.identity_state, after.advertised_profiles, after.open_handles
        ));
    } else if catalog_available {
        set_session_trace_id(None);
        info_line(&format!(
            "[crosspuck] catalog available without live bridge reason={reason} identity={:?} profiles={}",
            after.identity_state, after.advertised_profiles
        ));
    } else if before.bridge_connected {
        set_session_trace_id(None);
        error_line(&format!(
            "[crosspuck] bridge disconnected reason={reason} identity={:?} profiles={}",
            after.identity_state, after.advertised_profiles
        ));
    }

    *last = Some(state);
}
