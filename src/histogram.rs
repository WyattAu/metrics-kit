//! Cumulative histogram with fixed bucket bounds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::MetricsError;
use crate::padding::CachePadded;

/// Default latency bucket bounds in seconds, matching the Prometheus client
/// convention (adequate for sub-millisecond to two-minute services).
pub const DEFAULT_METRIC_BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0,
];

/// Builds `count` exponentially-spaced bucket bounds — the Prometheus
/// client `exponential_buckets` constructor, for feed-forward latencies
/// that the default latency ladder fits poorly.
///
/// Bound `i` is `start * factor^i` (so `bounds[0] == start` and
/// `bounds[count-1] == start * factor^(count-1)`). The result feeds
/// [`Registry::histogram_with_buckets`](crate::Registry::histogram_with_buckets).
///
/// # Errors
///
/// [`MetricsError::InvalidBuckets`] unless `count > 0`, `start > 0`,
/// `factor > 1.0`, and all three are finite; also if a computed bound
/// overflows to infinity.
pub fn exponential_buckets(
    start: f64,
    factor: f64,
    count: usize,
) -> Result<Vec<f64>, MetricsError> {
    if count == 0 {
        return Err(MetricsError::InvalidBuckets {
            reason: "count must be positive".to_string(),
        });
    }
    if !start.is_finite() || start <= 0.0 {
        return Err(MetricsError::InvalidBuckets {
            reason: "start must be finite and positive".to_string(),
        });
    }
    if !factor.is_finite() || factor <= 1.0 {
        return Err(MetricsError::InvalidBuckets {
            reason: "factor must be finite and greater than 1".to_string(),
        });
    }
    let mut bounds = Vec::with_capacity(count);
    let mut current = start;
    for _ in 0..count {
        if !current.is_finite() {
            return Err(MetricsError::InvalidBuckets {
                reason: "bucket bounds overflow to infinity".to_string(),
            });
        }
        bounds.push(current);
        current *= factor;
    }
    Ok(bounds)
}

/// Builds `count` linearly-spaced bucket bounds — the Prometheus client
/// `linear_buckets` constructor, for fixed-width ranges (status codes,
/// sizes, counts).
///
/// Bound `i` is `start + width * i`. The result feeds
/// [`Registry::histogram_with_buckets`](crate::Registry::histogram_with_buckets).
///
/// # Errors
///
/// [`MetricsError::InvalidBuckets`] unless `count > 0`, `start >= 0`,
/// `width > 0`, and all three are finite; also if a computed bound
/// overflows to infinity.
pub fn linear_buckets(start: f64, width: f64, count: usize) -> Result<Vec<f64>, MetricsError> {
    if count == 0 {
        return Err(MetricsError::InvalidBuckets {
            reason: "count must be positive".to_string(),
        });
    }
    if !start.is_finite() || start < 0.0 {
        return Err(MetricsError::InvalidBuckets {
            reason: "start must be finite and non-negative".to_string(),
        });
    }
    if !width.is_finite() || width <= 0.0 {
        return Err(MetricsError::InvalidBuckets {
            reason: "width must be finite and positive".to_string(),
        });
    }
    let mut bounds = Vec::with_capacity(count);
    for i in 0..count {
        let bound = start + width * i as f64;
        if !bound.is_finite() {
            return Err(MetricsError::InvalidBuckets {
                reason: "bucket bounds overflow to infinity".to_string(),
            });
        }
        bounds.push(bound);
    }
    Ok(bounds)
}

#[derive(Debug, Default)]
struct Inner {
    counts: Vec<CachePadded<AtomicU64>>,
    sum_bits: CachePadded<AtomicU64>,
    count: CachePadded<AtomicU64>,
}

/// A cumulative histogram: fixed upper bounds, per-bucket atomic counters,
/// plus running `_sum` and `_count`. Rendering emits `le` buckets, `+Inf`,
/// `_sum`, and `_count` per the Prometheus text format. Handles are cheap
/// to clone and share state.
#[derive(Debug, Clone)]
pub struct Histogram {
    buckets: Arc<[f64]>,
    inner: Arc<Inner>,
}

impl Histogram {
    pub(crate) fn with_buckets(buckets: Vec<f64>) -> Self {
        let counts = (0..buckets.len())
            .map(|_| CachePadded::new(AtomicU64::new(0)))
            .collect();
        Self {
            buckets: buckets.into(),
            inner: Arc::new(Inner {
                counts,
                sum_bits: CachePadded::new(AtomicU64::new(0)),
                count: CachePadded::new(AtomicU64::new(0)),
            }),
        }
    }

    pub(crate) fn bucket_bounds(&self) -> &[f64] {
        &self.buckets
    }

    /// Records one observation.
    ///
    /// Bucket selection is a linear scan over the bounds (14 comparisons at
    /// default sizing — branch-predictable and cheaper than a binary search
    /// at these widths). NaN observations are counted in `+Inf` only, never
    /// added to `_sum` (which would poison it).
    pub fn observe(&self, value: f64) {
        let mut idx = self.buckets.len();
        for (i, bound) in self.buckets.iter().enumerate() {
            if value <= *bound {
                idx = i;
                break;
            }
        }
        if let Some(cell) = self.inner.counts.get(idx) {
            cell.fetch_add(1, Ordering::Relaxed);
        }
        if !value.is_nan() {
            let mut current = f64::from_bits(self.inner.sum_bits.load(Ordering::Relaxed));
            loop {
                let updated = (current + value).to_bits();
                match self.inner.sum_bits.compare_exchange_weak(
                    current.to_bits(),
                    updated,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(observed) => current = f64::from_bits(observed),
                }
            }
        }
        self.inner.count.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn bucket_count(&self, idx: usize) -> u64 {
        self.inner
            .counts
            .get(idx)
            .map_or(0, |c| c.load(Ordering::Relaxed))
    }

    /// Cumulative count of observations with value `<= bounds[idx]`, as the
    /// exposition format requires (`le` semantics). Accumulated at render
    /// time so the hot path stays O(1).
    pub(crate) fn cumulative_count(&self, idx: usize) -> u64 {
        (0..=idx)
            .take_while(|i| *i < self.inner.counts.len())
            .map(|i| self.bucket_count(i))
            .sum()
    }

    pub(crate) fn sum(&self) -> f64 {
        f64::from_bits(self.inner.sum_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn count(&self) -> u64 {
        self.inner.count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn exponential_buckets_match_prometheus_client() {
        // The Prometheus client's canonical example: 4 buckets from 100,
        // factor 10.
        let bounds = exponential_buckets(100.0, 10.0, 4).expect("bounds");
        assert_eq!(bounds, vec![100.0, 1000.0, 10_000.0, 100_000.0]);
        let single = exponential_buckets(1.0, 2.0, 1).expect("bounds");
        assert_eq!(single, vec![1.0]);
    }

    #[test]
    fn linear_buckets_match_prometheus_client() {
        // The Prometheus client's canonical example: 5 buckets, start 0,
        // width 0.5.
        let bounds = linear_buckets(0.0, 0.5, 5).expect("bounds");
        assert_eq!(bounds, vec![0.0, 0.5, 1.0, 1.5, 2.0]);
    }

    #[test]
    fn bucket_constructors_reject_bad_parameters() {
        assert!(exponential_buckets(1.0, 2.0, 0).is_err());
        assert!(exponential_buckets(0.0, 2.0, 4).is_err());
        assert!(exponential_buckets(-1.0, 2.0, 4).is_err());
        assert!(
            exponential_buckets(1.0, 1.0, 4).is_err(),
            "factor must exceed 1"
        );
        assert!(exponential_buckets(1.0, f64::NAN, 4).is_err());
        // overflow: 1e300 * 1e300 per step
        assert!(exponential_buckets(1e300, 1e300, 3).is_err());

        assert!(linear_buckets(0.0, 1.0, 0).is_err());
        assert!(linear_buckets(-0.5, 1.0, 4).is_err());
        assert!(linear_buckets(0.0, 0.0, 4).is_err());
        assert!(linear_buckets(0.0, -1.0, 4).is_err());
        assert!(linear_buckets(f64::NAN, 1.0, 4).is_err());
        assert!(linear_buckets(1e308, 1e308, 3).is_err());
    }

    #[test]
    fn constructor_output_feeds_the_registry() {
        let r = crate::Registry::new();
        let bounds = exponential_buckets(1.0, 2.0, 3).expect("bounds");
        let h = r
            .histogram_with_buckets("exp_total_latency", "h", &[], bounds)
            .expect("register");
        h.observe(1.5);
        assert_eq!(h.bucket_count(0), 0);
        assert_eq!(h.bucket_count(1), 1);
    }

    #[test]
    fn observations_land_in_correct_buckets() {
        let h = Histogram::with_buckets(vec![1.0, 2.0, 5.0]);
        h.observe(0.5);
        h.observe(1.5);
        h.observe(4.0);
        h.observe(100.0); // +Inf bucket
        assert_eq!(h.bucket_count(0), 1);
        assert_eq!(h.bucket_count(1), 1);
        assert_eq!(h.bucket_count(2), 1);
        assert_eq!(h.count(), 4);
        assert_eq!(h.sum(), 106.0);
    }

    #[test]
    fn cumulative_buckets_are_monotonic() {
        let h = Histogram::with_buckets(DEFAULT_METRIC_BUCKETS.to_vec());
        h.observe(0.01);
        h.observe(3.0);
        for i in 1..h.bucket_bounds().len() {
            assert!(h.cumulative_count(i) >= h.cumulative_count(i - 1));
        }
        assert_eq!(h.cumulative_count(h.bucket_bounds().len() - 1), 2);
    }

    #[test]
    fn cloned_handles_share_state() {
        let h = Histogram::with_buckets(vec![1.0, 2.0]);
        let h2 = h.clone();
        h.observe(0.5);
        h2.observe(0.5);
        assert_eq!(h.count(), 2);
        assert_eq!(h2.bucket_count(0), 2);
    }

    #[test]
    fn nan_counts_but_never_poisons_sum() {
        let h = Histogram::with_buckets(vec![1.0]);
        h.observe(f64::NAN);
        assert_eq!(h.count(), 1);
        assert_eq!(h.sum(), 0.0);
    }

    #[test]
    fn concurrent_observations_are_lossless() {
        let h = Arc::new(Histogram::with_buckets(DEFAULT_METRIC_BUCKETS.to_vec()));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let h = Arc::clone(&h);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        h.observe(1.0);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread join");
        }
        assert_eq!(h.count(), 80_000);
        assert_eq!(h.sum(), 80_000.0);
    }
}
