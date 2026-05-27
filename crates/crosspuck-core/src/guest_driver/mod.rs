pub mod bridge;
pub mod config;
pub mod handles;
pub mod identity;
pub mod profile;
pub mod routing;
pub mod runtime;
pub mod trace;

pub use bridge::{HostBridge, HostBridgeConfig, HostBridgeError, HostBridgeInputStats};
pub use config::{GuestLogLevel, RuntimeConfig};
pub use handles::{VirtualHandleId, VirtualHandleTable};
pub use identity::{RuntimeIdentity, RuntimeIdentityState};
pub use profile::{
    path_may_be_virtual, HidCaps, VirtualHidProfile, VirtualHidProfileCatalog,
    VirtualHidProfileDescriptor,
};
pub use routing::{
    classify_device_path, classify_hid_query, hid_query_may_target_crosspuck, is_crosspuck_vid_pid,
    should_append_synthetic_to_setupapi_query, DevicePathRoute, HidQueryRoute, SetupApiQuery,
    CROSSPUCK_PRODUCT_ID, CROSSPUCK_VENDOR_ID,
};
pub use runtime::{GuestDriverError, GuestDriverRuntime, GuestDriverSnapshot};
pub use trace::TraceLimiter;
