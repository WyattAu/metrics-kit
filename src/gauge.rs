//! Instantaneous gauge.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::padding::CachePadded;

/// A value that can go up and down, stored as f64 bits in a cache-line
/// padded atomic. Handles are cheap to clone and share state.
#[derive(Debug, Clone)]
pub struct Gauge {
    inner: Arc<CachePadded<AtomicU64>>,
}

impl Gauge {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(CachePadded::new(AtomicU64::new(0))),
        }
    }

    /// Sets the gauge to `value`.
    pub fn set(&self, value: f64) {
        self.inner.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Adds `delta` to the gauge (delta may be negative).
    ///
    /// Uses a CAS loop; under contention each retry observes the freshest
    /// value, so no update is lost.
    pub fn add(&self, delta: f64) {
        let mut current = self.inner.load(Ordering::Relaxed);
        loop {
            let updated = (f64::from_bits(current) + delta).to_bits();
            match self.inner.compare_exchange_weak(
                current,
                updated,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// Reads the current value.
    pub fn get(&self) -> f64 {
        f64::from_bits(self.inner.load(Ordering::Relaxed))
    }

    pub(crate) fn snapshot(&self) -> f64 {
        self.get()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn set_and_get_roundtrip() {
        let g = Gauge::new();
        g.set(3.5);
        assert_eq!(g.get(), 3.5);
    }

    #[test]
    fn negative_delta_decrements() {
        let g = Gauge::new();
        g.set(10.0);
        g.add(-4.0);
        assert_eq!(g.get(), 6.0);
    }

    #[test]
    fn cloned_handles_share_state() {
        let g = Gauge::new();
        let g2 = g.clone();
        g.set(1.0);
        g2.set(2.0);
        assert_eq!(g.get(), 2.0);
    }

    #[test]
    fn concurrent_adds_are_lossless() {
        let g = Arc::new(Gauge::new());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let g = Arc::clone(&g);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        g.add(f64::from(i));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread join");
        }
        assert_eq!(g.get(), (0..8).map(f64::from).sum::<f64>() * 10_000.0);
    }
}
