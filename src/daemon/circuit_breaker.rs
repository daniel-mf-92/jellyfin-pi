use std::collections::HashMap;
use std::time::{Duration, Instant};

use log::warn;

/// In-memory circuit breaker: tracks restart attempts per component.
/// If a component exceeds `max_restarts` within `window`, further restarts
/// are blocked until older entries age out.
///
/// Replaces the on-disk `$STATE_DIR/cb_*` files from the shell scripts.
pub struct CircuitBreaker {
    records: HashMap<String, Vec<Instant>>,
    window: Duration,
    default_max: usize,
}

impl CircuitBreaker {
    pub fn new(default_max: usize) -> Self {
        Self {
            records: HashMap::new(),
            window: Duration::from_secs(3600), // 1 hour
            default_max,
        }
    }

    /// Returns `true` if the component is allowed to restart (breaker closed).
    /// Returns `false` if max restarts exceeded in the window (breaker open).
    pub fn check(&mut self, component: &str, max_restarts: Option<usize>) -> bool {
        let max = max_restarts.unwrap_or(self.default_max);
        let now = Instant::now();
        let cutoff = now - self.window;

        // Prune old entries
        if let Some(entries) = self.records.get_mut(component) {
            entries.retain(|&t| t > cutoff);
            if entries.len() >= max {
                warn!(
                    "Circuit breaker OPEN for '{}': {} restarts in last hour (max {})",
                    component,
                    entries.len(),
                    max
                );
                return false;
            }
        }

        true
    }

    /// Record a restart for the component.
    pub fn record(&mut self, component: &str) {
        self.records
            .entry(component.to_string())
            .or_default()
            .push(Instant::now());
    }

    /// Check and record in one call. Returns true if allowed (and records it).
    pub fn try_restart(&mut self, component: &str, max_restarts: Option<usize>) -> bool {
        if self.check(component, max_restarts) {
            self.record(component);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_within_limit() {
        let mut cb = CircuitBreaker::new(3);
        assert!(cb.try_restart("test", None));
        assert!(cb.try_restart("test", None));
        assert!(cb.try_restart("test", None));
        // 4th should be blocked
        assert!(!cb.check("test", None));
    }

    #[test]
    fn separate_components() {
        let mut cb = CircuitBreaker::new(1);
        assert!(cb.try_restart("a", None));
        assert!(!cb.check("a", None));
        // "b" is independent
        assert!(cb.try_restart("b", None));
    }

    #[test]
    fn custom_max() {
        let mut cb = CircuitBreaker::new(3);
        assert!(cb.try_restart("test", Some(1)));
        assert!(!cb.check("test", Some(1)));
    }
}
