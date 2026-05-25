use crate::protocol::{CollectionDescriptor, CollectionRole, IdentityPayload};

pub const FALLBACK_VENDOR_ID: u16 = 0x28DE;
pub const FALLBACK_PRODUCT_ID: u16 = 0x1304;
pub const FALLBACK_VERSION_NUMBER: u16 = 0x0002;
pub const FALLBACK_MANUFACTURER: &str = "Valve Software";
pub const FALLBACK_PRODUCT: &str = "Steam Controller Puck";
pub const FALLBACK_SERIAL: &str = "FXB9961303C9C";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeIdentity {
    host: Option<IdentityPayload>,
    fallback: Option<IdentityPayload>,
    stale: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeIdentityState {
    Missing,
    Fallback,
    Live,
    Stale,
}

impl RuntimeIdentity {
    pub fn new(allow_debug_fallback: bool) -> Self {
        Self {
            host: None,
            fallback: allow_debug_fallback.then(default_fallback_identity),
            stale: false,
        }
    }

    pub fn set_host_identity(&mut self, identity: IdentityPayload) {
        self.host = Some(identity);
        self.stale = false;
    }

    pub fn mark_stale(&mut self) {
        if self.host.is_some() {
            self.stale = true;
        }
    }

    pub fn clear_host_identity(&mut self) {
        self.host = None;
        self.stale = false;
    }

    pub fn state(&self) -> RuntimeIdentityState {
        if self.host.is_some() && self.stale {
            RuntimeIdentityState::Stale
        } else if self.host.is_some() {
            RuntimeIdentityState::Live
        } else if self.fallback.is_some() {
            RuntimeIdentityState::Fallback
        } else {
            RuntimeIdentityState::Missing
        }
    }

    pub fn live_identity(&self) -> Option<&IdentityPayload> {
        (!self.stale).then_some(self.host.as_ref()).flatten()
    }

    pub fn identity(&self) -> Option<&IdentityPayload> {
        self.live_identity().or(self.fallback.as_ref())
    }

    pub fn can_advertise(&self, host_bridge_required: bool) -> bool {
        if host_bridge_required {
            self.live_identity().is_some()
        } else {
            self.identity().is_some()
        }
    }
}

impl Default for RuntimeIdentity {
    fn default() -> Self {
        Self::new(false)
    }
}

pub fn default_fallback_identity() -> IdentityPayload {
    IdentityPayload {
        vendor_id: FALLBACK_VENDOR_ID,
        product_id: FALLBACK_PRODUCT_ID,
        version_number: FALLBACK_VERSION_NUMBER,
        manufacturer: FALLBACK_MANUFACTURER.to_string(),
        product: FALLBACK_PRODUCT.to_string(),
        serial: FALLBACK_SERIAL.to_string(),
        collections: vec![
            fallback_collection(CollectionRole::PuckMain, 2, 0x0001, 0x0001, 54, 64, 64),
            fallback_collection(
                CollectionRole::PuckInterface3,
                3,
                0x0001,
                0x0001,
                54,
                64,
                64,
            ),
            fallback_collection(
                CollectionRole::PuckInterface4,
                4,
                0x0001,
                0x0001,
                54,
                64,
                64,
            ),
            fallback_collection(
                CollectionRole::PuckInterface5,
                5,
                0x0001,
                0x0001,
                54,
                64,
                64,
            ),
            fallback_collection(
                CollectionRole::PuckVendorDongle,
                6,
                0xFF00,
                0x0002,
                54,
                1,
                64,
            ),
        ],
    }
}

fn fallback_collection(
    role: CollectionRole,
    interface_number: u8,
    usage_page: u16,
    usage: u16,
    input_report_len: u16,
    output_report_len: u16,
    feature_report_len: u16,
) -> CollectionDescriptor {
    CollectionDescriptor {
        role,
        interface_number,
        usage_page,
        usage,
        input_report_len,
        output_report_len,
        feature_report_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_mode_does_not_advertise_fallback() {
        let identity = RuntimeIdentity::new(true);

        assert_eq!(identity.state(), RuntimeIdentityState::Fallback);
        assert!(identity.can_advertise(false));
        assert!(!identity.can_advertise(true));
    }

    #[test]
    fn stale_host_identity_is_not_live() {
        let mut identity = RuntimeIdentity::new(false);
        identity.set_host_identity(default_fallback_identity());
        assert_eq!(identity.state(), RuntimeIdentityState::Live);

        identity.mark_stale();

        assert_eq!(identity.state(), RuntimeIdentityState::Stale);
        assert!(identity.live_identity().is_none());
        assert!(!identity.can_advertise(true));
    }
}
