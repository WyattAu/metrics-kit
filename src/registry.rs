//! The metric registry: registration, validation, and exposition rendering.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::counter::Counter;
use crate::error::MetricsError;
use crate::histogram::Histogram;
use crate::{exposition::render_series, gauge::Gauge};

pub use crate::histogram::DEFAULT_METRIC_BUCKETS;

/// Default cardinality budget: 1,024 series per registry.
pub const DEFAULT_MAX_SERIES: usize = 1024;

pub(crate) enum MetricKind {
    Counter(Counter),
    Gauge(Gauge),
    Histogram(Histogram),
}

pub(crate) struct Series {
    pub(crate) name: String,
    pub(crate) labels: Vec<(String, String)>,
    pub(crate) kind: MetricKind,
}

pub(crate) struct Family {
    pub(crate) help: String,
    pub(crate) series: Vec<Arc<Series>>,
}

/// Collection of registered metrics.
///
/// Registration takes the registry lock — intended at startup and
/// reconfiguration only; the hot path touches only the returned metric
/// handles and never locks. [`Registry::render`] is called on the scrape
/// path. Iteration order is deterministic (`BTreeMap` by family name) so
/// scrapes diff cleanly.
#[derive(Default)]
pub struct Registry {
    inner: RwLock<RegistryState>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("series_count", &self.series_count())
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct RegistryState {
    families: BTreeMap<String, Family>,
    series_count: usize,
    max_series: usize,
}

impl Registry {
    /// Creates an empty registry with the default cardinality budget of
    /// [`DEFAULT_MAX_SERIES`] series.
    pub fn new() -> Self {
        Self::with_max_series(DEFAULT_MAX_SERIES)
    }

    /// Creates an empty registry with an explicit cardinality budget.
    ///
    /// The budget caps total series (metric + unique label-set pairs) to
    /// protect VictoriaMetrics/Grafana from cardinality explosions.
    pub fn with_max_series(max_series: usize) -> Self {
        Self {
            inner: RwLock::new(RegistryState {
                families: BTreeMap::new(),
                series_count: 0,
                max_series,
            }),
        }
    }

    /// Registers a counter and returns a shared handle for the hot path.
    ///
    /// # Errors
    ///
    /// - [`MetricsError::InvalidMetricName`] / [`MetricsError::InvalidLabelName`]
    ///   on malformed identifiers.
    /// - [`MetricsError::CardinalityLimit`] when the budget is exhausted.
    /// - [`MetricsError::DuplicateSeries`] when the exact (name, labels)
    ///   pair already exists.
    /// - [`MetricsError::RegistryPoisoned`] when a prior concurrent
    ///   registration panicked; treat as fatal.
    pub fn counter(
        &self,
        name: &str,
        help: &str,
        labels: &[(&str, &str)],
    ) -> Result<Counter, MetricsError> {
        let counter = Counter::new();
        let labels = validated(name, labels)?;
        let mut state = self.lock()?;
        self.check_budget(&state, name)?;
        let family = entry(&mut state, name, help);
        if family.series.iter().any(|s| s.labels == labels) {
            return Err(MetricsError::DuplicateSeries {
                name: name.to_string(),
            });
        }
        family.series.push(Arc::new(Series {
            name: name.to_string(),
            labels,
            kind: MetricKind::Counter(counter.clone()),
        }));
        state.series_count += 1;
        Ok(counter)
    }

    /// Registers a gauge. See [`Registry::counter`] for the error contract.
    pub fn gauge(
        &self,
        name: &str,
        help: &str,
        labels: &[(&str, &str)],
    ) -> Result<Gauge, MetricsError> {
        let gauge = Gauge::new();
        let labels = validated(name, labels)?;
        let mut state = self.lock()?;
        self.check_budget(&state, name)?;
        let family = entry(&mut state, name, help);
        if family.series.iter().any(|s| s.labels == labels) {
            return Err(MetricsError::DuplicateSeries {
                name: name.to_string(),
            });
        }
        family.series.push(Arc::new(Series {
            name: name.to_string(),
            labels,
            kind: MetricKind::Gauge(gauge.clone()),
        }));
        state.series_count += 1;
        Ok(gauge)
    }

    /// Registers a histogram with the default latency buckets. See
    /// [`Registry::counter`] for the error contract.
    pub fn histogram(
        &self,
        name: &str,
        help: &str,
        labels: &[(&str, &str)],
    ) -> Result<Histogram, MetricsError> {
        self.histogram_with_buckets(name, help, labels, DEFAULT_METRIC_BUCKETS.to_vec())
    }

    /// Registers a histogram with explicit bucket bounds, which must be
    /// non-empty and strictly ascending.
    ///
    /// # Errors
    ///
    /// In addition to the [`Registry::counter`] error contract, returns
    /// [`MetricsError::InvalidBuckets`] for empty or non-ascending bounds.
    pub fn histogram_with_buckets(
        &self,
        name: &str,
        help: &str,
        labels: &[(&str, &str)],
        buckets: Vec<f64>,
    ) -> Result<Histogram, MetricsError> {
        if buckets.is_empty() {
            return Err(MetricsError::InvalidBuckets {
                reason: "bucket bounds are empty".to_string(),
            });
        }
        if buckets
            .iter()
            .zip(buckets.iter().skip(1))
            .any(|(a, b)| a >= b)
        {
            return Err(MetricsError::InvalidBuckets {
                reason: "bucket bounds must be strictly ascending".to_string(),
            });
        }
        let histogram = Histogram::with_buckets(buckets);
        let labels = validated(name, labels)?;
        let mut state = self.lock()?;
        self.check_budget(&state, name)?;
        let family = entry(&mut state, name, help);
        if family.series.iter().any(|s| s.labels == labels) {
            return Err(MetricsError::DuplicateSeries {
                name: name.to_string(),
            });
        }
        family.series.push(Arc::new(Series {
            name: name.to_string(),
            labels,
            kind: MetricKind::Histogram(histogram.clone()),
        }));
        state.series_count += 1;
        Ok(histogram)
    }

    /// Renders the full registry in Prometheus text exposition format 0.0.4,
    /// ready to serve at `/metrics`. Returns an empty string if the registry
    /// lock is poisoned (see [`MetricsError::RegistryPoisoned`]; a poisoned
    /// registry must not serve metrics).
    pub fn render(&self) -> String {
        let Ok(state) = self.inner.read() else {
            return String::new();
        };
        let mut out = String::with_capacity(1024 + state.series_count * 96);
        for family in state.families.values() {
            for series in &family.series {
                render_series(&mut out, family.help.as_str(), series);
            }
        }
        out
    }

    /// Number of registered series (for budget monitoring and tests).
    pub fn series_count(&self) -> usize {
        self.inner.read().map(|s| s.series_count).unwrap_or(0)
    }

    fn lock(&self) -> Result<std::sync::RwLockWriteGuard<'_, RegistryState>, MetricsError> {
        self.inner
            .write()
            .map_err(|_| MetricsError::RegistryPoisoned)
    }

    fn check_budget(&self, state: &RegistryState, name: &str) -> Result<(), MetricsError> {
        if state.series_count >= state.max_series {
            return Err(MetricsError::CardinalityLimit {
                name: name.to_string(),
                limit: state.max_series,
            });
        }
        Ok(())
    }
}

fn entry<'a>(state: &'a mut RegistryState, name: &str, help: &str) -> &'a mut Family {
    state
        .families
        .entry(name.to_string())
        .or_insert_with(|| Family {
            help: help.to_string(),
            series: Vec::new(),
        })
}

fn validated(name: &str, labels: &[(&str, &str)]) -> Result<Vec<(String, String)>, MetricsError> {
    validate_name(name)?;
    let mut out: Vec<(String, String)> = Vec::with_capacity(labels.len());
    for (k, v) in labels {
        let valid = !k.is_empty()
            && k.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(MetricsError::InvalidLabelName {
                name: (*k).to_string(),
            });
        }
        out.push(((*k).to_string(), (*v).to_string()));
    }
    out.sort();
    Ok(out)
}

fn validate_name(name: &str) -> Result<(), MetricsError> {
    let valid = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':');
    if valid {
        Ok(())
    } else {
        Err(MetricsError::InvalidMetricName {
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn counter_render_includes_value() {
        let r = Registry::new();
        let c = r.counter("demo_total", "Demo.", &[]).expect("register");
        c.inc();
        c.inc();
        let text = r.render();
        assert!(text.contains("# TYPE demo_total counter"));
        assert!(text.contains("demo_total 2"), "got: {text}");
    }

    #[test]
    fn hot_path_and_registry_share_state() {
        let r = Registry::new();
        let c = r.counter("shared_total", "h", &[]).expect("register");
        c.inc();
        assert!(r.render().contains("shared_total 1"));
    }

    #[test]
    fn duplicate_series_rejected() {
        let r = Registry::new();
        r.counter("dup_total", "h", &[]).expect("first");
        let err = r.counter("dup_total", "h", &[]).expect_err("duplicate");
        assert!(matches!(err, MetricsError::DuplicateSeries { .. }));
    }

    #[test]
    fn same_name_different_labels_ok() {
        let r = Registry::new();
        r.counter("http_total", "h", &[("method", "GET")])
            .expect("get");
        r.counter("http_total", "h", &[("method", "POST")])
            .expect("post");
        assert_eq!(r.series_count(), 2);
        let text = r.render();
        assert!(text.contains(r#"http_total{method="GET"} 0"#));
        assert!(text.contains(r#"http_total{method="POST"} 0"#));
    }

    #[test]
    fn invalid_names_rejected() {
        let r = Registry::new();
        assert!(r.counter("9bad", "h", &[]).is_err());
        assert!(r.counter("has space", "h", &[]).is_err());
        assert!(r.counter("ok_name", "h", &[("9bad", "v")]).is_err());
    }

    #[test]
    fn cardinality_budget_enforced() {
        let r = Registry::with_max_series(2);
        r.counter("a_total", "h", &[]).expect("a");
        r.counter("b_total", "h", &[]).expect("b");
        let err = r.counter("c_total", "h", &[]).expect_err("over budget");
        assert!(matches!(err, MetricsError::CardinalityLimit { .. }));
    }

    #[test]
    fn histogram_render_is_valid_exposition() {
        let r = Registry::new();
        let h = r
            .histogram("demo_duration_us", "Duration.", &[])
            .expect("register");
        h.observe(0.25);
        h.observe(3.0);
        let text = r.render();
        assert!(text.contains("# TYPE demo_duration_us histogram"));
        assert!(text.contains(r#"demo_duration_us_bucket{le="0.5"} 1"#));
        assert!(text.contains(r#"demo_duration_us_bucket{le="+Inf"} 2"#));
        assert!(text.contains("demo_duration_us_sum 3.25"));
        assert!(text.contains("demo_duration_us_count 2"));
    }

    #[test]
    fn non_ascending_buckets_rejected() {
        let r = Registry::new();
        assert!(r
            .histogram_with_buckets("h_bad", "h", &[], vec![1.0, 1.0])
            .is_err());
        assert!(r
            .histogram_with_buckets("h_bad2", "h", &[], vec![])
            .is_err());
    }

    #[test]
    fn render_is_deterministic_across_calls() {
        let r = Registry::new();
        r.counter("b_total", "h", &[]).expect("b");
        r.counter("a_total", "h", &[]).expect("a");
        assert_eq!(r.render(), r.render());
        let text = r.render();
        assert!(text.find("a_total").unwrap() < text.find("b_total").unwrap());
    }
}
