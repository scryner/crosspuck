use super::log::{debug_line, set_session_trace_id};
use crosspuck_core::guest_driver::{
    GuestDriverRuntime, GuestDriverSnapshot, RuntimeConfig, RuntimeIdentityState,
    VirtualHidProfileCatalog,
};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

static RUNTIME: OnceLock<GuestDriverRuntime> = OnceLock::new();
static BRIDGE_CONNECT_ALLOWED: OnceLock<bool> = OnceLock::new();
static LAST_CATALOG_LOG_STATE: OnceLock<Mutex<Option<CatalogLogState>>> = OnceLock::new();
static BRIDGE_CONNECTOR_STARTED: OnceLock<()> = OnceLock::new();

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

    start_bridge_connector(reason);
    let before = runtime.snapshot();
    let catalog = match runtime.catalog_result() {
        Ok(catalog) => catalog,
        Err(error) => {
            set_session_trace_id(runtime.session_trace_id());
            debug_line(&format!(
                "[crosspuck] lazy bridge connect failed reason={reason}: {error}"
            ));
            return None;
        }
    };
    let after = runtime.snapshot();
    log_catalog_state(reason, &before, &after, catalog.is_some());
    catalog
}

pub fn start_bridge_connector(reason: &'static str) {
    if !bridge_connect_allowed() {
        return;
    }

    if BRIDGE_CONNECTOR_STARTED.set(()).is_err() {
        return;
    }

    thread::spawn(move || {
        for attempt in 1..=8 {
            let Some(runtime) = runtime() else {
                return;
            };
            let snapshot = runtime.snapshot();
            if snapshot.bridge_connected {
                set_session_trace_id(snapshot.session_trace_id);
                debug_line(&format!(
                    "[crosspuck] background bridge connector already connected reason={reason} identity={:?} profiles={}",
                    snapshot.identity_state, snapshot.advertised_profiles
                ));
                return;
            }

            debug_line(&format!(
                "[crosspuck] background bridge connect attempt={attempt}/8 reason={reason}"
            ));
            match runtime.connect_bridge() {
                Ok(()) => {
                    set_session_trace_id(runtime.session_trace_id());
                    let snapshot = runtime.snapshot();
                    debug_line(&format!(
                        "[crosspuck] background bridge connect ok reason={reason} identity={:?} profiles={} open_handles={}",
                        snapshot.identity_state, snapshot.advertised_profiles, snapshot.open_handles
                    ));
                    return;
                }
                Err(error) => {
                    set_session_trace_id(runtime.session_trace_id());
                    debug_line(&format!(
                        "[crosspuck] background bridge connect failed attempt={attempt}/8 reason={reason}: {error}"
                    ));
                }
            }

            thread::sleep(Duration::from_millis(500));
        }
    });
}

pub fn catalog_if_connected(reason: &'static str) -> Option<VirtualHidProfileCatalog> {
    let runtime = runtime()?;
    let catalog = runtime.catalog_if_connected();
    if catalog.is_none() {
        let snapshot = runtime.snapshot();
        debug_line(&format!(
            "[crosspuck] cached catalog unavailable reason={reason} bridge_connected={} identity={:?} profiles={}",
            snapshot.bridge_connected, snapshot.identity_state, snapshot.advertised_profiles
        ));
    }
    catalog
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
    if !changed && !(catalog_available && !before.bridge_connected && after.bridge_connected) {
        return;
    }

    if after.bridge_connected {
        set_session_trace_id(after.session_trace_id);
        debug_line(&format!(
            "[crosspuck] lazy bridge connect ok reason={reason} identity={:?} profiles={} open_handles={}",
            after.identity_state, after.advertised_profiles, after.open_handles
        ));
    } else if catalog_available {
        set_session_trace_id(None);
        debug_line(&format!(
            "[crosspuck] catalog available without live bridge reason={reason} identity={:?} profiles={}",
            after.identity_state, after.advertised_profiles
        ));
    } else if before.bridge_connected {
        set_session_trace_id(None);
        debug_line(&format!(
            "[crosspuck] bridge disconnected reason={reason} identity={:?} profiles={}",
            after.identity_state, after.advertised_profiles
        ));
    }

    *last = Some(state);
}
