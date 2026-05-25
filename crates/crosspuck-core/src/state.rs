use crate::hid::{snapshot_for_filter, HidFilter, HidSnapshotError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceState {
    PuckDisconnected,
    PuckConnected { serial: String },
    GuestProxyConnected { serial: String, guest: String },
}

impl ServiceState {
    pub fn menu_label(&self) -> String {
        match self {
            Self::PuckDisconnected => "Puck disconnected".to_string(),
            Self::PuckConnected { serial } => format!("Puck connected ({serial})"),
            Self::GuestProxyConnected { serial, guest } => {
                format!("Guest proxy connected ({serial}, {guest})")
            }
        }
    }
}

pub fn snapshot_host_state() -> Result<ServiceState, HidSnapshotError> {
    let snapshot = snapshot_for_filter(&HidFilter::steam_puck())?;
    Ok(ServiceState::PuckConnected {
        serial: snapshot.identity.serial,
    })
}
