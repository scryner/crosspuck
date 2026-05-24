#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceLimiter {
    enabled: bool,
    limit: usize,
    max_bytes: usize,
    count: usize,
}

impl TraceLimiter {
    pub fn new(enabled: bool, limit: usize, max_bytes: usize) -> Self {
        Self {
            enabled,
            limit,
            max_bytes,
            count: 0,
        }
    }

    pub fn should_trace(&mut self) -> bool {
        if !self.enabled || self.count >= self.limit {
            return false;
        }
        self.count += 1;
        true
    }

    pub fn render_bytes(&self, bytes: &[u8]) -> String {
        let shown = bytes.len().min(self.max_bytes);
        let mut rendered = bytes[..shown]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        if bytes.len() > shown {
            rendered.push_str(&format!(" ...(+{} bytes)", bytes.len() - shown));
        }
        rendered
    }

    pub fn traced_count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_after_limit() {
        let mut limiter = TraceLimiter::new(true, 2, 8);

        assert!(limiter.should_trace());
        assert!(limiter.should_trace());
        assert!(!limiter.should_trace());
    }

    #[test]
    fn renders_bounded_hex() {
        let limiter = TraceLimiter::new(true, 1, 2);

        assert_eq!(
            limiter.render_bytes(&[0x80, 0x00, 0xAA]),
            "80 00 ...(+1 bytes)"
        );
    }
}
