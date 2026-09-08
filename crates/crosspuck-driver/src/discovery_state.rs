//! Tracks acknowledged discovery notifications. Failed deliveries (including a
//! not-yet-created SDL window) remain pending until the next worker iteration.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeviceChange {
    Arrival(u64),
    Removal,
}

#[derive(Default)]
pub(crate) struct DiscoveryState {
    announced: Option<u64>,
}

impl DiscoveryState {
    pub(crate) fn pending(&self, live: Option<u64>) -> Option<DeviceChange> {
        if self.announced == live {
            None
        } else {
            Some(match live {
                Some(generation) => DeviceChange::Arrival(generation),
                None => DeviceChange::Removal,
            })
        }
    }

    pub(crate) fn acknowledge(&mut self, change: DeviceChange) {
        self.announced = match change {
            DeviceChange::Arrival(generation) => Some(generation),
            DeviceChange::Removal => None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_host_and_missing_receiver_retry_until_delivery() {
        let mut state = DiscoveryState::default();
        assert_eq!(state.pending(None), None);
        let change = DeviceChange::Arrival(1);
        assert_eq!(state.pending(Some(1)), Some(change));
        // No acknowledgement: SDL has not created its window yet.
        assert_eq!(state.pending(Some(1)), Some(change));
        state.acknowledge(change);
        assert_eq!(state.pending(Some(1)), None);
    }

    #[test]
    fn disconnect_and_replacement_each_prompt_discovery() {
        let mut state = DiscoveryState::default();
        state.acknowledge(DeviceChange::Arrival(1));
        assert_eq!(state.pending(None), Some(DeviceChange::Removal));
        state.acknowledge(DeviceChange::Removal);
        assert_eq!(state.pending(None), None);
        assert_eq!(state.pending(Some(2)), Some(DeviceChange::Arrival(2)));
        state.acknowledge(DeviceChange::Arrival(2));
        // An input call can reconnect between worker polls.
        assert_eq!(state.pending(Some(3)), Some(DeviceChange::Arrival(3)));
    }

    #[test]
    fn obsolete_undelivered_arrival_is_not_sent_after_host_disappears() {
        let state = DiscoveryState::default();
        assert_eq!(state.pending(Some(1)), Some(DeviceChange::Arrival(1)));
        assert_eq!(state.pending(None), None);
    }
}
