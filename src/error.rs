//! Errors produced during metric registration.

use std::fmt;

/// Registration and rendering failures.
///
/// Documented failure modes are part of the API contract; every variant
/// lists the condition that produces it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetricsError {
    /// A metric with this exact (name, label set) pair is already
    /// registered.
    DuplicateSeries {
        /// The colliding metric name.
        name: String,
    },
    /// Registering this series would exceed the registry's cardinality
    /// budget ([`Registry::with_max_series`](crate::Registry::with_max_series)).
    CardinalityLimit {
        /// The metric name that hit the budget.
        name: String,
        /// The configured budget.
        limit: usize,
    },
    /// The metric name is not a valid Prometheus identifier
    /// (`[a-zA-Z_:][a-zA-Z0-9_:]*`).
    InvalidMetricName {
        /// The rejected name.
        name: String,
    },
    /// A label name is not a valid Prometheus identifier.
    InvalidLabelName {
        /// The rejected label name.
        name: String,
    },
    /// Histogram bucket bounds were empty or not strictly ascending.
    InvalidBuckets {
        /// Human-readable reason.
        reason: String,
    },
    /// The registry's internal lock was poisoned by a panic during a
    /// concurrent registration. Treat as fatal: the registry contents are
    /// indeterminate and must not serve traffic.
    RegistryPoisoned,
}

impl fmt::Display for MetricsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSeries { name } => {
                write!(f, "duplicate metric series: {name}")
            }
            Self::CardinalityLimit { name, limit } => {
                write!(f, "cardinality limit ({limit}) reached registering: {name}")
            }
            Self::InvalidMetricName { name } => {
                write!(f, "invalid metric name: {name:?}")
            }
            Self::InvalidLabelName { name } => {
                write!(f, "invalid label name: {name:?}")
            }
            Self::InvalidBuckets { reason } => {
                write!(f, "invalid histogram buckets: {reason}")
            }
            Self::RegistryPoisoned => {
                write!(f, "metric registry lock poisoned; registry is unusable")
            }
        }
    }
}

impl std::error::Error for MetricsError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn display_is_informative() {
        let e = MetricsError::CardinalityLimit {
            name: "x_total".into(),
            limit: 10,
        };
        assert_eq!(
            e.to_string(),
            "cardinality limit (10) reached registering: x_total"
        );
        assert_eq!(
            MetricsError::RegistryPoisoned.to_string(),
            "metric registry lock poisoned; registry is unusable"
        );
    }
}
