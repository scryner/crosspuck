use crate::protocol::IdentityPayload;

pub const CROSSPUCK_VENDOR_ID: u16 = 0x28de;
pub const CROSSPUCK_PRODUCT_ID: u16 = 0x1304;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HidQueryRoute {
    AppendCrossPuckSynthetic,
    NativeOnly,
}

impl HidQueryRoute {
    pub fn appends_synthetic(self) -> bool {
        matches!(self, Self::AppendCrossPuckSynthetic)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevicePathRoute {
    CrossPuckCandidate,
    NativeOnly,
}

impl DevicePathRoute {
    pub fn is_crosspuck_candidate(self) -> bool {
        matches!(self, Self::CrossPuckCandidate)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetupApiQuery {
    pub hid_interface_class: bool,
    pub empty_enumerator: bool,
}

pub fn classify_device_path(path: &str) -> DevicePathRoute {
    let lower = path.to_ascii_lowercase();
    if lower.contains("hid#") && lower.contains("vid_28de") && lower.contains("pid_1304") {
        DevicePathRoute::CrossPuckCandidate
    } else {
        DevicePathRoute::NativeOnly
    }
}

pub fn classify_hid_query(
    vendor_id: u16,
    product_id: u16,
    identity: &IdentityPayload,
) -> HidQueryRoute {
    if !hid_query_may_target_crosspuck(vendor_id, product_id) {
        return HidQueryRoute::NativeOnly;
    }

    if vendor_id == 0 && product_id == 0 {
        return HidQueryRoute::AppendCrossPuckSynthetic;
    }

    if vendor_id == identity.vendor_id && (product_id == 0 || product_id == identity.product_id) {
        HidQueryRoute::AppendCrossPuckSynthetic
    } else {
        HidQueryRoute::NativeOnly
    }
}

pub fn hid_query_may_target_crosspuck(vendor_id: u16, product_id: u16) -> bool {
    (vendor_id == 0 && product_id == 0)
        || (vendor_id == CROSSPUCK_VENDOR_ID
            && (product_id == 0 || product_id == CROSSPUCK_PRODUCT_ID))
}

pub fn is_crosspuck_vid_pid(vendor_id: u16, product_id: u16, identity: &IdentityPayload) -> bool {
    vendor_id == identity.vendor_id && product_id == identity.product_id
}

pub fn should_append_synthetic_to_setupapi_query(query: SetupApiQuery) -> bool {
    query.hid_interface_class && query.empty_enumerator
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest_driver::identity::default_fallback_identity;

    #[test]
    fn classifies_only_crosspuck_vid_pid_paths_as_candidates() {
        assert_eq!(
            classify_device_path(r"\\?\hid#vid_28de&pid_1304&mi_02#serial"),
            DevicePathRoute::CrossPuckCandidate
        );
        assert_eq!(
            classify_device_path(r"\\?\hid#vid_045e&pid_0b13#serial"),
            DevicePathRoute::NativeOnly
        );
        assert_eq!(
            classify_device_path(r"\\?\hid#vid_046d&pid_c52b#serial"),
            DevicePathRoute::NativeOnly
        );
        assert_eq!(
            classify_device_path(r"\\?\hid#vid_845e&pid_0001#serial"),
            DevicePathRoute::NativeOnly
        );
        assert_eq!(
            classify_device_path(r"\\.\pipe\steam"),
            DevicePathRoute::NativeOnly
        );
    }

    #[test]
    fn classifies_only_broad_or_crosspuck_hid_queries_for_synthetic_append() {
        let identity = default_fallback_identity();

        assert_eq!(
            classify_hid_query(0, 0, &identity),
            HidQueryRoute::AppendCrossPuckSynthetic
        );
        assert_eq!(
            classify_hid_query(CROSSPUCK_VENDOR_ID, CROSSPUCK_PRODUCT_ID, &identity),
            HidQueryRoute::AppendCrossPuckSynthetic
        );
        assert_eq!(
            classify_hid_query(CROSSPUCK_VENDOR_ID, 0, &identity),
            HidQueryRoute::AppendCrossPuckSynthetic
        );
        assert_eq!(
            classify_hid_query(0, CROSSPUCK_PRODUCT_ID, &identity),
            HidQueryRoute::NativeOnly
        );
        assert_eq!(
            classify_hid_query(0x045e, 0x0b13, &identity),
            HidQueryRoute::NativeOnly
        );
        assert_eq!(
            classify_hid_query(0x046d, 0xc52b, &identity),
            HidQueryRoute::NativeOnly
        );
    }

    #[test]
    fn prefilters_hid_queries_without_identity_or_runtime_state() {
        assert!(hid_query_may_target_crosspuck(0, 0));
        assert!(hid_query_may_target_crosspuck(CROSSPUCK_VENDOR_ID, 0));
        assert!(hid_query_may_target_crosspuck(
            CROSSPUCK_VENDOR_ID,
            CROSSPUCK_PRODUCT_ID
        ));
        assert!(!hid_query_may_target_crosspuck(0, CROSSPUCK_PRODUCT_ID));
        assert!(!hid_query_may_target_crosspuck(0x045e, 0x0b13));
        assert!(!hid_query_may_target_crosspuck(0x046d, 0xc52b));
    }

    #[test]
    fn checks_exact_crosspuck_vid_pid() {
        let identity = default_fallback_identity();

        assert!(is_crosspuck_vid_pid(
            CROSSPUCK_VENDOR_ID,
            CROSSPUCK_PRODUCT_ID,
            &identity
        ));
        assert!(!is_crosspuck_vid_pid(CROSSPUCK_VENDOR_ID, 0, &identity));
        assert!(!is_crosspuck_vid_pid(0, CROSSPUCK_PRODUCT_ID, &identity));
        assert!(!is_crosspuck_vid_pid(0x045e, 0x0b13, &identity));
    }

    #[test]
    fn setupapi_append_requires_hid_class_and_empty_enumerator() {
        assert!(should_append_synthetic_to_setupapi_query(SetupApiQuery {
            hid_interface_class: true,
            empty_enumerator: true,
        }));
        assert!(!should_append_synthetic_to_setupapi_query(SetupApiQuery {
            hid_interface_class: false,
            empty_enumerator: true,
        }));
        assert!(!should_append_synthetic_to_setupapi_query(SetupApiQuery {
            hid_interface_class: true,
            empty_enumerator: false,
        }));
    }
}
