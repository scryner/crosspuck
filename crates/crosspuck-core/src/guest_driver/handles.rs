use super::profile::VirtualHidProfile;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VirtualHandleId(u64);

impl VirtualHandleId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct VirtualHandleTable {
    next_id: u64,
    handles: HashMap<VirtualHandleId, VirtualHidProfile>,
    open_counts: HashMap<VirtualHidProfile, usize>,
}

impl VirtualHandleTable {
    pub fn open(&mut self, profile: VirtualHidProfile) -> VirtualHandleId {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = VirtualHandleId(self.next_id);
        self.handles.insert(id, profile);
        *self.open_counts.entry(profile).or_default() += 1;
        id
    }

    pub fn close(&mut self, id: VirtualHandleId) -> Option<usize> {
        let profile = self.handles.remove(&id)?;
        let count = self.open_counts.entry(profile).or_default();
        *count = count.saturating_sub(1);
        Some(*count)
    }

    pub fn profile(&self, id: VirtualHandleId) -> Option<VirtualHidProfile> {
        self.handles.get(&id).copied()
    }

    pub fn is_open(&self, id: VirtualHandleId) -> bool {
        self.handles.contains_key(&id)
    }

    pub fn open_count(&self, profile: VirtualHidProfile) -> usize {
        self.open_counts.get(&profile).copied().unwrap_or(0)
    }

    pub fn total_open_count(&self) -> usize {
        self.handles.len()
    }

    pub fn clear(&mut self) {
        self.handles.clear();
        self.open_counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_open_close_per_profile() {
        let mut table = VirtualHandleTable::default();

        let first = table.open(VirtualHidProfile::Main);
        let second = table.open(VirtualHidProfile::Main);
        let vendor = table.open(VirtualHidProfile::VendorDongle);

        assert_ne!(first, second);
        assert_eq!(table.profile(vendor), Some(VirtualHidProfile::VendorDongle));
        assert_eq!(table.open_count(VirtualHidProfile::Main), 2);
        assert_eq!(table.total_open_count(), 3);

        assert_eq!(table.close(first), Some(1));
        assert_eq!(table.open_count(VirtualHidProfile::Main), 1);
        assert!(!table.is_open(first));
        assert!(table.is_open(second));
    }
}
