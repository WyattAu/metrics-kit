//! Exemplar integration tests: last-writer-wins storage rendered as
//! OpenMetrics exemplar suffixes, ignored by the legacy format. Compiles
//! to nothing without the `exemplars` + `openmetrics` features.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(all(feature = "exemplars", feature = "openmetrics"))]
mod gated {
    use metrics_kit::{Encoder, Format, NamePolicy, Registry};

    #[test]
    fn exemplar_renders_after_the_sample_in_openmetrics() {
        let registry = Registry::new();
        let requests = registry
            .counter("demo_requests_total", "Total demo requests.", &[])
            .expect("register");
        requests.with_exemplar(&[], 1, "trace_id", "7b3f");

        let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
        assert!(
            body.contains(r#"demo_requests_total 1 # {trace_id="7b3f"} 1.0"#),
            "{body}"
        );
    }

    #[test]
    fn exemplar_last_writer_wins_across_scrapes() {
        let registry = Registry::new();
        let requests = registry
            .counter("demo_requests_total", "Total demo requests.", &[])
            .expect("register");
        requests.with_exemplar(&[("span", "root")], 1, "trace_id", "111111");
        requests.with_exemplar(&[("span", "child")], 2, "trace_id", "222222");

        let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
        assert!(
            body.contains(r#"demo_requests_total 3 # {trace_id="222222",span="child"} 2.0"#),
            "newest exemplar must win: {body}"
        );
        assert!(
            !body.contains("111111"),
            "replaced exemplar must not linger: {body}"
        );
        // The exemplar value is the last recorded value, not the total.
        assert!(body.contains("} 2.0"), "{body}");
    }

    #[test]
    fn exemplars_are_not_rendered_in_prom_text() {
        let registry = Registry::new();
        let requests = registry
            .counter("demo_requests_total", "Total demo requests.", &[])
            .expect("register");
        requests.with_exemplar(&[], 1, "trace_id", "7b3f");

        let body = registry.render();
        assert!(body.contains("demo_requests_total 1\n"), "{body}");
        assert!(!body.contains("trace_id"), "{body}");
    }

    #[test]
    fn clear_exemplar_drops_the_suffix() {
        let registry = Registry::new();
        let requests = registry
            .counter("demo_requests_total", "Total demo requests.", &[])
            .expect("register");
        requests.with_exemplar(&[], 1, "trace_id", "7b3f");
        requests.clear_exemplar();

        let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
        assert!(body.contains("demo_requests_total 1\n"), "{body}");
        assert!(!body.contains("7b3f"), "{body}");
    }

    #[test]
    fn long_trace_ids_truncate_but_render() {
        let registry = Registry::new();
        let requests = registry
            .counter("demo_requests_total", "Total demo requests.", &[])
            .expect("register");
        let long_id = "f".repeat(500);
        requests.with_exemplar(&[], 1, "trace_id", &long_id);

        let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
        assert!(
            body.contains("demo_requests_total 1 # {trace_id=\""),
            "{body}"
        );
        assert!(
            !body.contains(&long_id),
            "500 f's must be truncated to the slot capacity: {body}"
        );
    }

    #[test]
    fn concurrent_exemplar_writers_stay_consistent() {
        let registry = std::sync::Arc::new(Registry::new());
        let requests = registry
            .counter("demo_requests_total", "Total demo requests.", &[])
            .expect("register");

        let handles: Vec<_> = (0..8)
            .map(|worker| {
                let requests = requests.clone();
                std::thread::spawn(move || {
                    let trace = format!("worker-{worker}");
                    for i in 0..2_000u64 {
                        requests.with_exemplar(&[], i, "trace_id", &trace);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("join");
        }

        // A snapshot after quiescence must be a whole stored exemplar,
        // never torn: exactly one worker's trace id, with that worker's
        // last value, and every recorded value still counted.
        let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
        let line = body
            .lines()
            .find(|l| l.starts_with("demo_requests_total "))
            .expect("sample line");
        assert!(
            line.contains(r#"trace_id="worker-"#),
            "snapshot must be a whole stored exemplar, never torn: {line}"
        );
        assert!(line.ends_with("} 1999.0"), "{line}");
        assert!(
            line.starts_with("demo_requests_total 15992000 "),
            "every value is still counted: {line}"
        );
    }

    #[test]
    fn exemplar_plus_labels_and_utf8_series_coexist() {
        let registry = Registry::new().with_name_policy(NamePolicy::Utf8);
        let requests = registry
            .counter("service.http.requests", "Dotted.", &[("route", "/")])
            .expect("register");
        requests.with_exemplar(&[], 5, "trace_id", "abc");

        let body = Encoder::new(Format::OpenMetrics).encode_to_string(&registry);
        assert!(
            body.contains(r#"{"service.http.requests_total",route="/"} 5 # {trace_id="abc"} 5.0"#),
            "quoted UTF-8 sample with exemplar: {body}"
        );
    }
}
