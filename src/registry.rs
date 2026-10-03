//! The metric registry: registration, validation, and exposition rendering.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::counter::Counter;
use crate::encoder::Format;
use crate::error::MetricsError;
#[cfg(feature = "openmetrics")]
use crate::exposition::render_family_om;
use crate::exposition::render_family_prom;
use crate::gauge::Gauge;
use crate::histogram::Histogram;

pub use crate::histogram::DEFAULT_METRIC_BUCKETS;

/// Maximum metric-name length accepted under
/// [`NamePolicy::Utf8`](NamePolicy::Utf8), as a sanity cap.
const MAX_UTF8_NAME_BYTES: usize = 255;

/// Default cardinality budget: 1,024 series per registry.
pub const DEFAULT_MAX_SERIES: usize = 1024;

/// Metric-name validation policy.
///
/// `Legacy` (the default) accepts only Prometheus identifiers
/// (`[a-zA-Z_:][a-zA-Z0-9_:]*`). `Utf8` additionally accepts any non-empty
/// UTF-8 name without control characters (≤255 bytes) for OpenMetrics
/// UTF-8 metric names; those series render quoted in OpenMetrics mode and
/// are omitted from legacy Prometheus-text renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum NamePolicy {
    /// Legacy Prometheus identifier names only (default).
    #[default]
    Legacy,
    /// Additionally accept OpenMetrics UTF-8 metric names.
    Utf8,
}

pub(crate) enum MetricKind {
    Counter(Counter),
    Gauge(Gauge),
    Histogram(Histogram),
}

pub(crate) struct Series {
    pub(crate) name: String,
    pub(crate) labels: Vec<(String, String)>,
    pub(crate) kind: MetricKind,
    /// Registration time in seconds since the Unix epoch, rendered as the
    /// OpenMetrics `_created` series (counters and histograms).
    #[cfg_attr(not(feature = "openmetrics"), allow(dead_code))]
    pub(crate) created: f64,
}

pub(crate) struct Family {
    pub(crate) help: String,
    pub(crate) series: Vec<Arc<Series>>,
}

/// Collection of registered metrics.
///
/// Registration takes the registry lock — intended at startup and
/// reconfiguration only; the hot path touches only the returned metric
/// handles and never locks. [`Registry::render`] and [`Registry::render_as`]
/// are called on the scrape path. Iteration order is deterministic
/// (`BTreeMap` by family name) so scrapes diff cleanly.
#[derive(Default)]
pub struct Registry {
    inner: RwLock<RegistryState>,
    name_policy: NamePolicy,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("series_count", &self.series_count())
            .field("name_policy", &self.name_policy)
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
            name_policy: NamePolicy::Legacy,
        }
    }

    /// Sets the metric-name validation policy (builder style).
    ///
    /// ```
    /// use metrics_kit::{NamePolicy, Registry};
    ///
    /// let registry = Registry::new().with_name_policy(NamePolicy::Utf8);
    /// assert!(registry
    ///     .counter("service.rust.http.requests", "Dotted name.", &[])
    ///     .is_ok());
    /// ```
    pub fn with_name_policy(mut self, name_policy: NamePolicy) -> Self {
        self.name_policy = name_policy;
        self
    }

    /// The active metric-name validation policy.
    pub fn name_policy(&self) -> NamePolicy {
        self.name_policy
    }

    /// The process-global registry, initialized exactly once on first use
    /// with the default cardinality budget and [`NamePolicy::Legacy`].
    ///
    /// Every caller observes the same `'static` registry for the life of
    /// the process — the telemetry-init pattern: concurrent first calls
    /// race to initialize, exactly one registry is constructed, and all
    /// callers block on that single result. Prefer an owned [`Registry`]
    /// in libraries and tests; the global exists for application wiring
    /// that cannot thread a handle everywhere.
    pub fn global() -> &'static Registry {
        static GLOBAL_REGISTRY: OnceLock<Registry> = OnceLock::new();
        GLOBAL_REGISTRY.get_or_init(Registry::new)
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
        let labels = validated(name, labels, self.name_policy)?;
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
            created: unix_now(),
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
        let labels = validated(name, labels, self.name_policy)?;
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
            created: unix_now(),
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
    /// non-empty and strictly ascending. Construct bounds with
    /// [`exponential_buckets`](crate::exponential_buckets) /
    /// [`linear_buckets`](crate::linear_buckets) for Prometheus-client
    /// parity.
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
        let labels = validated(name, labels, self.name_policy)?;
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
            created: unix_now(),
        }));
        state.series_count += 1;
        Ok(histogram)
    }

    /// Renders the full registry in Prometheus text exposition format 0.0.4,
    /// ready to serve at `/metrics`. Returns an empty string if the registry
    /// lock is poisoned (see [`MetricsError::RegistryPoisoned`]; a poisoned
    /// registry must not serve metrics).
    pub fn render(&self) -> String {
        self.render_as(Format::PromText)
    }

    /// Renders the full registry in `format`. Each metric family renders
    /// under one `# HELP`/`# TYPE` header pair; OpenMetrics output
    /// additionally terminates with `# EOF` (requires the `openmetrics`
    /// feature). Series registered with UTF-8 names under
    /// [`NamePolicy::Utf8`] render quoted in OpenMetrics and are omitted
    /// from Prometheus-text output, which cannot represent them. Returns
    /// an empty string if the registry lock is poisoned.
    pub fn render_as(&self, format: Format) -> String {
        let Ok(state) = self.inner.read() else {
            return String::new();
        };
        let mut out = String::with_capacity(1024 + state.series_count * 96);
        for family in state.families.values() {
            match format {
                Format::PromText => {
                    render_family_prom(&mut out, family.help.as_str(), &family.series);
                }
                #[cfg(feature = "openmetrics")]
                Format::OpenMetrics => {
                    render_family_om(&mut out, family.help.as_str(), &family.series);
                }
            }
        }
        #[cfg(feature = "openmetrics")]
        if format == Format::OpenMetrics {
            out.push_str("# EOF\n");
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

/// Seconds since the Unix epoch at registration (the OpenMetrics
/// `_created` value). Clocks before the epoch degrade to `0.0`.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
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

/// Whether `name` is a legacy Prometheus identifier
/// (`[a-zA-Z_:][a-zA-Z0-9_:]*`). Shared with the exposition renderer,
/// which quotes non-legacy names in OpenMetrics output.
pub(crate) fn is_legacy_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn validated(
    name: &str,
    labels: &[(&str, &str)],
    policy: NamePolicy,
) -> Result<Vec<(String, String)>, MetricsError> {
    validate_name(name, policy)?;
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

fn validate_name(name: &str, policy: NamePolicy) -> Result<(), MetricsError> {
    let valid = match policy {
        NamePolicy::Legacy => is_legacy_name(name),
        NamePolicy::Utf8 => {
            !name.is_empty()
                && name.len() <= MAX_UTF8_NAME_BYTES
                && !name.chars().any(char::is_control)
        }
    };
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

    #[test]
    fn family_headers_render_once_per_family() {
        let r = Registry::new();
        r.counter("fam_total", "h", &[("m", "GET")]).expect("get");
        r.counter("fam_total", "h", &[("m", "POST")]).expect("post");
        let text = r.render();
        assert_eq!(text.matches("# TYPE fam_total counter").count(), 1);
        assert_eq!(text.matches("# HELP fam_total h").count(), 1);
    }

    #[test]
    fn render_matches_prom_text_encoder() {
        let r = Registry::new();
        r.counter("enc_total", "h", &[]).expect("register");
        assert_eq!(r.render(), r.render_as(Format::PromText));
    }

    #[test]
    fn global_registry_initializes_once() {
        let a = Registry::global();
        let b = Registry::global();
        assert!(
            std::ptr::eq(a, b),
            "every caller must observe the same process-global registry"
        );
        // The global accepts registrations and renders them, and the
        // registration is visible through the other handle — proving
        // init-once shared state rather than per-call construction.
        let registered_before = a.series_count();
        a.counter("global_probe_total", "Global probe.", &[])
            .or_else(|_| b.counter("global_probe_total", "Global probe.", &[]))
            .expect("register on the global");
        assert_eq!(b.series_count(), registered_before + 1);
        assert!(a.render().contains("global_probe_total"));
    }

    #[test]
    fn utf8_policy_accepts_dotted_names() {
        let r = Registry::new().with_name_policy(NamePolicy::Utf8);
        assert_eq!(r.name_policy(), NamePolicy::Utf8);
        assert!(r.counter("service.http.requests", "h", &[]).is_ok());
        assert!(r.counter("", "empty", &[]).is_err());
        assert!(r.counter("bad\nname", "control char", &[]).is_err());
        let long = "x".repeat(256);
        assert!(r.counter(&long, "too long", &[]).is_err());
        assert!(r.counter(&"x".repeat(255), "at cap", &[]).is_ok());
    }

    #[test]
    fn utf8_names_rejected_under_legacy_policy() {
        let r = Registry::new();
        assert_eq!(r.name_policy(), NamePolicy::Legacy);
        assert!(r.counter("service.http.requests", "h", &[]).is_err());
    }

    #[test]
    fn labels_stay_legacy_validated_under_utf8_policy() {
        let r = Registry::new().with_name_policy(NamePolicy::Utf8);
        assert!(r.counter("ok.name", "h", &[("dotted.label", "v")]).is_err());
    }
}
