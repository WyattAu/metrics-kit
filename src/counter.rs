//! Monotonic counter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::padding::CachePadded;

/// A cumulative counter that only ever increases (and resets to zero on
/// process restart, per Prometheus semantics).
///
/// Handles are cheap to clone and share the same underlying atomic, so the
/// registration-time handle and the hot-path handle observe one series.
/// Recording is a relaxed `fetch_add` on a cache-line-padded atomic. With
/// the `exemplars` feature the handle also owns a fixed-capacity exemplar
/// slot (see [`Counter::with_exemplar`]).
#[derive(Debug, Clone)]
pub struct Counter {
    inner: Arc<CachePadded<AtomicU64>>,
    #[cfg(feature = "exemplars")]
    exemplar: Arc<CachePadded<crate::exemplar::ExemplarSlot>>,
}

impl Counter {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(CachePadded::new(AtomicU64::new(0))),
            #[cfg(feature = "exemplars")]
            exemplar: Arc::new(CachePadded::new(crate::exemplar::ExemplarSlot::new())),
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

    /// Records `value` and attaches an exemplar — trace context rendered
    /// alongside the sample in OpenMetrics mode as
    /// `# {trace_id="..."} 1.0` after the sample value.
    ///
    /// `exemplar_key`/`exemplar_val` are the primary exemplar label pair
    /// (typically the trace id); `labels` contributes up to
    /// [`MAX_EXEMPLAR_EXTRA_PAIRS`](crate::MAX_EXEMPLAR_EXTRA_PAIRS) additional context pairs.
    /// `value` is both added to the counter and stored as the exemplar's
    /// observed value.
    ///
    /// Storage is last-writer-wins: the slot holds one exemplar per series
    /// and the newest recording replaces the previous one. The call is
    /// lock-free and allocation-free — a fixed-capacity seqlock slot, like
    /// the estate's percentile ring; label strings longer than
    /// [`MAX_EXEMPLAR_STR_BYTES`](crate::MAX_EXEMPLAR_STR_BYTES) bytes are truncated at a
    /// UTF-8 char boundary.
    ///
    /// # Example
    ///
    /// Rendered in OpenMetrics mode (requires the `openmetrics` feature):
    #[cfg_attr(all(feature = "exemplars", feature = "openmetrics"), doc = "```")]
    #[cfg_attr(
        not(all(feature = "exemplars", feature = "openmetrics")),
        doc = "```ignore"
    )]
    /// use metrics_kit::{Encoder, Format, Registry};
    ///
    /// let registry = Registry::new();
    /// let requests = registry
    ///     .counter("demo_requests_total", "Total demo requests", &[])
    ///     .expect("unique name");
    /// requests.with_exemplar(&[("span", "root")], 1, "trace_id", "7b3f");
    ///
    /// let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
    /// assert!(body.contains(
    ///     r#"demo_requests_total 1 # {trace_id="7b3f",span="root"} 1.0"#
    /// ));
    /// ```
    #[cfg(feature = "exemplars")]
    pub fn with_exemplar(
        &self,
        labels: &[(&str, &str)],
        value: u64,
        exemplar_key: &str,
        exemplar_val: &str,
    ) {
        self.add(value);
        let pairs: [(&str, &str); crate::exemplar::MAX_PAIRS] = std::array::from_fn(|i| {
            if i == 0 {
                (exemplar_key, exemplar_val)
            } else {
                labels.get(i - 1).copied().unwrap_or(("", ""))
            }
        });
        let n = 1 + labels.len().min(crate::exemplar::MAX_EXEMPLAR_EXTRA_PAIRS);
        self.exemplar.store(pairs.as_slice(), n, value as f64);
    }

    /// Drops the stored exemplar so the next scrape renders the series
    /// without one.
    #[cfg(feature = "exemplars")]
    pub fn clear_exemplar(&self) {
        self.exemplar.clear();
    }

    /// Reads the current value. Intended for tests and the scrape path;
    /// hot code should never read counters.
    pub fn get(&self) -> u64 {
        self.inner.load(Ordering::Relaxed)
    }

    pub(crate) fn snapshot(&self) -> u64 {
        self.get()
    }

    #[cfg(feature = "exemplars")]
    pub(crate) fn snapshot_exemplar(&self) -> Option<crate::exemplar::ExemplarSnapshot> {
        self.exemplar.load()
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

    #[cfg(feature = "exemplars")]
    #[test]
    fn exemplar_records_and_clears() {
        let c = Counter::new();
        assert!(c.snapshot_exemplar().is_none());
        c.with_exemplar(&[("span", "root")], 3, "trace_id", "abc");
        let snap = c.snapshot_exemplar().expect("exemplar");
        assert_eq!(snap.value, 3.0);
        assert_eq!(c.get(), 3);
        assert_eq!(snap.labels.len(), 2);
        assert_eq!(
            snap.labels.first(),
            Some(&("trace_id".to_string(), "abc".to_string()))
        );
        c.clear_exemplar();
        assert!(c.snapshot_exemplar().is_none());
    }

    #[cfg(feature = "exemplars")]
    #[test]
    fn exemplar_last_writer_wins() {
        let c = Counter::new();
        c.with_exemplar(&[], 1, "trace_id", "old");
        c.with_exemplar(&[], 2, "trace_id", "new");
        let snap = c.snapshot_exemplar().expect("exemplar");
        assert_eq!(snap.labels.first().map(|p| p.1.as_str()), Some("new"));
        assert_eq!(snap.value, 2.0);
        assert_eq!(c.get(), 3, "every value is still counted");
    }
}
