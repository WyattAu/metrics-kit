//! Lock-free Prometheus-exposition metrics kit.
//!
//! `metrics-kit` centralizes the hot-path metrics pattern used across the
//! WyattAu estate: cache-line-padded lock-free counters, gauges, and
//! histograms that are registered once at startup and rendered in the
//! [Prometheus text exposition format 0.0.4] — directly scrapeable by
//! VictoriaMetrics, Grafana Agent, vmagent, or any Prometheus-compatible
//! collector.
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
//!   feature adds a ready-made `/metrics` route.
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
//! # Labels
//!
//! Label sets are fixed at registration; each unique (metric, label set)
//! pair gets its own series. The registry enforces a cardinality budget so
//! a runaway loop cannot explode VictoriaMetrics series counts.
//!
//! [Prometheus text exposition format 0.0.4]:
//!     https://prometheus.io/docs/instrumenting/exposition_formats/

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod counter;
mod error;
mod exposition;
mod gauge;
mod histogram;
mod padding;
mod registry;

pub use counter::Counter;
pub use error::MetricsError;
pub use gauge::Gauge;
pub use histogram::Histogram;
pub use padding::CachePadded;
pub use registry::{Registry, DEFAULT_MAX_SERIES, DEFAULT_METRIC_BUCKETS};

#[cfg(feature = "axum")]
mod axum_route;
#[cfg(feature = "axum")]
pub use axum_route::metrics_route;
