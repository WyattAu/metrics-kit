//! Lock-free Prometheus/OpenMetrics-exposition metrics kit.
//!
//! `metrics-kit` centralizes the hot-path metrics pattern used across the
//! WyattAu estate: cache-line-padded lock-free counters, gauges, and
//! histograms that are registered once at startup and rendered in the
//! [Prometheus text exposition format 0.0.4] or the [OpenMetrics text
//! format 1.0.0] — directly scrapeable by VictoriaMetrics, Grafana Agent,
//! vmagent, or any Prometheus-compatible collector.
//!
//! # Design
//!
//! - **Hot path is lock-free.** Recording touches only `AtomicU64`s with
//!   relaxed ordering, padded to 64-byte cache lines to prevent false
//!   sharing between workers.
//! - **Registration is fallible.** Metric names and label sets are
//!   validated; duplicate names and cardinality explosions are rejected
//!   with a typed error instead of silently corrupting the exposition.
//! - **Zero dependencies in the default build.** The optional `axum`
//!   feature adds a ready-made `/metrics` route; the `openmetrics` and
//!   `exemplars` features are also dependency-free.
//!
//! # Example
//!
//! ```
//! use metrics_kit::{Counter, Registry};
//!
//! let registry = Registry::new();
//! let requests: Counter = registry
//!     .counter("demo_requests_total", "Total demo requests", &[])
//!     .expect("unique name");
//!
//! requests.inc();
//!
//! let text = registry.render();
//! assert!(text.contains("demo_requests_total 1"));
//! ```
//!
//! # Formats and negotiation
//!
//! [`Registry::render`] (or `Encoder::new(Format::PromText)`) always
//! produces the legacy 0.0.4 text format. With the `openmetrics` feature,
//! `Encoder::new(Format::OpenMetrics)` /
//! [`Registry::render_as`] produce OpenMetrics 1.0.0 — a `# EOF`
//! terminator, `_created` series, quoted UTF-8 metric names (opt in via
//! [`NamePolicy::Utf8`]), and exemplars when the
//! `exemplars` feature is also on — and [`Format::content_type`] supplies
//! the matching response header so scrapers negotiate via `Accept`.
//! Series registered under [`NamePolicy::Utf8`]
//! names render quoted in OpenMetrics and are omitted from legacy
//! Prometheus text, which cannot represent them.
//! # Exemplars
//!
//! With the `exemplars` feature, counters carry a fixed-capacity,
//! lock-free last-writer-wins exemplar slot
//! ([`Counter::with_exemplar`]); the newest recording wins and the record
//! path stays allocation-free. Exemplars render only in OpenMetrics mode.
//!
//! # Process-global registry
//!
//! [`Registry::global`] hands out the process-wide registry, initialized
//! exactly once (the telemetry-init pattern) for wiring that cannot
//! thread a handle everywhere.
//!
//! # Bucket constructors
//!
//! [`exponential_buckets`] and [`linear_buckets`] build ladder bounds with
//! Prometheus-client semantics for feeding
//! [`Registry::histogram_with_buckets`].
//!
//! # Summaries and quantiles
//!
//! There is deliberately **no summary / quantile sketch in this crate in
//! v0.2** (no DDSketch, no streaming quantiles): histogram `le` buckets
//! plus the estate's `percentile-kit` cover estimation, and a wrong
//! sketch silently poisons every SLO built on it. If a later version
//! adds one, it will ship behind its own feature with published error
//! bounds.
//!
//! # Labels
//!
//! Label sets are fixed at registration; each unique (metric, label set)
//! pair gets its own series. The registry enforces a cardinality budget so
//! a runaway loop cannot explode VictoriaMetrics series counts.
//!
//! [Prometheus text exposition format 0.0.4]:
//!     https://prometheus.io/docs/instrumenting/exposition_formats/
//! [OpenMetrics text format 1.0.0]:
//!     https://openmetrics.io/

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod counter;
mod encoder;
mod error;
#[cfg(feature = "exemplars")]
mod exemplar;
mod exposition;
mod gauge;
mod histogram;
mod padding;
mod registry;

pub use counter::Counter;
pub use encoder::{Encoder, Format};
pub use error::MetricsError;
#[cfg(feature = "exemplars")]
pub use exemplar::{MAX_EXEMPLAR_EXTRA_PAIRS, MAX_EXEMPLAR_STR_BYTES};
pub use gauge::Gauge;
pub use histogram::{exponential_buckets, linear_buckets, Histogram};
pub use padding::CachePadded;
pub use registry::{NamePolicy, Registry, DEFAULT_MAX_SERIES, DEFAULT_METRIC_BUCKETS};

#[cfg(feature = "axum")]
mod axum_route;
#[cfg(feature = "axum")]
pub use axum_route::metrics_route;
