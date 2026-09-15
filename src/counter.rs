//! Monotonic counter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::padding::CachePadded;

/// A cumulative counter that only ever increases (and resets to zero on
/// process restart, per Prometheus semantics).
///
/// Handles are cheap to clone and share the same underlying atomic, so the
/// registration-time handle and the hot-path handle observe one series.
/// Recording is a relaxed `fetch_add` on a cache-line-padded atomic.
#[derive(Debug, Clone)]
pub struct Counter {
    inner: Arc<CachePadded<AtomicU64>>,
}

impl Counter {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(CachePadded::new(AtomicU64::new(0))),
        }
    }

    /// Adds `value` to the counter. Counters are monotonic; a `value` of
    /// zero is accepted but has no effect on the exposition.
    pub fn add(&self, value: u64) {
        self.inner.fetch_add(value, Ordering::Relaxed);
    }

    /// Increments the counter by one.
    pub fn inc(&self) {
        self.add(1);
    }

    /// Reads the current value. Intended for tests and the scrape path;
    /// hot code should never read counters.
    pub fn get(&self) -> u64 {
        self.inner.load(Ordering::Relaxed)
    }

    pub(crate) fn snapshot(&self) -> u64 {
        self.get()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn increments_and_reads() {
        let c = Counter::new();
        c.inc();
        c.add(41);
        assert_eq!(c.get(), 42);
    }

    #[test]
    fn cloned_handles_share_state() {
        let c = Counter::new();
        let c2 = c.clone();
        c.inc();
        c2.add(9);
        assert_eq!(c.get(), 10);
        assert_eq!(c2.get(), 10);
    }

    #[test]
    fn concurrent_adds_do_not_lose_updates() {
        let c = Arc::new(Counter::new());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = Arc::clone(&c);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        c.inc();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread join");
        }
        assert_eq!(c.get(), 80_000);
    }
}
