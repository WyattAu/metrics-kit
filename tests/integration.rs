#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests: the full register → record → scrape surface.

use metrics_kit::{Counter, Gauge, Histogram, Registry};
use std::sync::Arc;

/// A realistic service-shape registry: startup registers, workers record,
/// the scraper renders. Mirrors how vane/clawdius-style services consume
/// the kit.
#[test]
fn service_lifecycle() {
    let registry = Arc::new(Registry::with_max_series(256));

    // Startup: fixed families.
    let requests: Counter = registry
        .counter(
            "demo_http_requests_total",
            "Total HTTP requests.",
            &[("method", "GET")],
        )
        .expect("register requests");
    let inflight: Gauge = registry
        .gauge("demo_http_inflight", "In-flight requests.", &[])
        .expect("register inflight");
    let latency: Histogram = registry
        .histogram(
            "demo_http_request_duration_seconds",
            "Request duration.",
            &[],
        )
        .expect("register latency");

    // Hot path: concurrent workers. 0.25 is binary-exact, so the partial
    // sums are order-independent and the rendered sum is deterministic.
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let requests = requests.clone();
            let inflight = inflight.clone();
            let latency = latency.clone();
            std::thread::spawn(move || {
                for _ in 0..250 {
                    inflight.add(1.0);
                    requests.inc();
                    latency.observe(0.25);
                    inflight.add(-1.0);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("join");
    }

    // Scrape path.
    let text = registry.render();
    assert!(text.contains(r#"demo_http_requests_total{method="GET"} 1000"#));
    assert!(text.contains("demo_http_inflight 0"));
    assert!(text.contains(r#"demo_http_request_duration_seconds_count 1000"#));
    assert!(text.contains("demo_http_request_duration_seconds_sum 250"));
    assert!(text.contains(r#"demo_http_request_duration_seconds_bucket{le="0.25"} 1000"#));
}

#[test]
fn exposition_is_parseable_line_by_line() {
    let registry = Registry::new();
    registry
        .counter(
            "c_total",
            "Has \"quotes\" and \\ backslash.",
            &[("k", "v\nx")],
        )
        .expect("register");
    registry.gauge("g", "Negative.", &[]).expect("register");
    let text = registry.render();

    for line in text.lines() {
        assert!(
            !line.contains('\r'),
            "CR found: exposition must use LF line endings"
        );
        if line.starts_with('#') {
            assert!(line.starts_with("# HELP ") || line.starts_with("# TYPE "));
        } else {
            // sample lines end in a numeric token
            let last = line.rsplit(' ').next().unwrap_or_default();
            let parsed: Result<f64, _> = last.parse();
            assert!(parsed.is_ok(), "non-numeric sample tail in {line:?}");
        }
    }
    assert!(text.contains("# HELP c_total Has \"quotes\" and \\\\ backslash."));
    assert!(text.contains(r#"c_total{k="v\nx"} 0"#));
}
