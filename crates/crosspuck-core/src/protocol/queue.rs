use super::{CollectionRole, InputReport};
use std::collections::VecDeque;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedInputReport {
    pub sequence: u32,
    pub interface_number: u8,
    pub role: CollectionRole,
    pub host_monotonic_us: u64,
    pub data: Vec<u8>,
}

impl QueuedInputReport {
    pub fn from_wire(sequence: u32, report: InputReport) -> Self {
        Self {
            sequence,
            interface_number: report.interface_number,
            role: report.role,
            host_monotonic_us: report.host_monotonic_us,
            data: report.data,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputQueueStats {
    pub pushed: u64,
    pub popped: u64,
    pub dropped_oldest: u64,
}

#[derive(Clone, Debug)]
pub struct InputReportQueue {
    capacity: usize,
    reports: VecDeque<QueuedInputReport>,
    stats: InputQueueStats,
}

impl InputReportQueue {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "input report queue capacity must be non-zero");
        Self {
            capacity,
            reports: VecDeque::with_capacity(capacity),
            stats: InputQueueStats::default(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn push(&mut self, report: QueuedInputReport) {
        if self.reports.len() == self.capacity {
            self.reports.pop_front();
            self.stats.dropped_oldest += 1;
        }
        self.reports.push_back(report);
        self.stats.pushed += 1;
    }

    pub fn pop(&mut self) -> Option<QueuedInputReport> {
        let report = self.reports.pop_front();
        if report.is_some() {
            self.stats.popped += 1;
        }
        report
    }

    pub fn clear(&mut self) {
        self.reports.clear();
    }

    pub fn len(&self) -> usize {
        self.reports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }

    pub fn stats(&self) -> InputQueueStats {
        self.stats
    }
}

impl Default for InputReportQueue {
    fn default() -> Self {
        Self::with_capacity(super::DEFAULT_INPUT_QUEUE_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(sequence: u32) -> QueuedInputReport {
        QueuedInputReport {
            sequence,
            interface_number: 2,
            role: CollectionRole::PuckMain,
            host_monotonic_us: sequence as u64,
            data: vec![sequence as u8],
        }
    }

    #[test]
    fn drops_oldest_report_on_overflow() {
        let mut queue = InputReportQueue::with_capacity(2);

        queue.push(report(1));
        queue.push(report(2));
        queue.push(report(3));

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.stats().dropped_oldest, 1);
        assert_eq!(queue.pop().unwrap().sequence, 2);
        assert_eq!(queue.pop().unwrap().sequence, 3);
    }
}
