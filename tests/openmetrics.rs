//! OpenMetrics 1.0.0 integration tests: golden document, `_total` /
//! `_created` naming, quoted UTF-8 names, `# EOF` termination, and
//! content types. Compiles to nothing without the `openmetrics` feature.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(feature = "openmetrics")]
mod gated {
    use metrics_kit::{Encoder, Format, NamePolicy, Registry};

    /// Splits a rendered document into lines and asserts the non-`_created`
    /// lines match the golden document exactly (order included), while
    /// `_created` lines must carry a plausible registration timestamp.
    fn assert_golden(rendered: &str, golden_without_timestamps: &str) {
        let mut rendered_iter = rendered.lines().peekable();
        for expected in golden_without_timestamps.lines() {
            let actual = rendered_iter.next().unwrap_or_else(|| {
                panic!("document ended before expected line: {expected:?}");
            });
            if expected
                .split('{')
                .next()
                .unwrap_or_default()
                .contains("_created")
            {
                assert_eq!(
                    actual.split(' ').next(),
                    expected.split(' ').next(),
                    "created-series name mismatch"
                );
                let ts: f64 = actual
                    .rsplit(' ')
                    .next()
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or_else(|e| panic!("created value not a float in {actual:?}: {e}"));
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_secs_f64();
                assert!(
                    (now - 120.0..=now + 120.0).contains(&ts),
                    "created timestamp {ts} not within 2 minutes of now {now}"
                );
            } else {
                assert_eq!(actual, expected, "golden line mismatch");
            }
        }
        let rest: Vec<&str> = rendered_iter.collect();
        assert!(rest.is_empty(), "unexpected trailing lines: {rest:?}");
    }

    #[test]
    fn openmetrics_golden_document() {
        let registry = Registry::new();
        let requests = registry
            .counter(
                "demo_requests_total",
                "Total demo requests.",
                &[("method", "GET")],
            )
            .expect("register counter");
        requests.inc();
        requests.inc();
        requests.inc();
        registry
            .gauge("demo_inflight", "In-flight.", &[])
            .expect("register gauge")
            .set(2.5);
        let duration = registry
            .histogram_with_buckets("demo_duration", "Duration.", &[], vec![0.5])
            .expect("register histogram");
        duration.observe(0.25);
        duration.observe(3.0);

        let body = registry.render_as(Format::OpenMetrics);
        assert_golden(
            &body,
            r#"# HELP demo_duration Duration.
# TYPE demo_duration histogram
demo_duration_bucket{le="0.5"} 1
demo_duration_bucket{le="+Inf"} 2
demo_duration_sum 3.25
demo_duration_count 2
demo_duration_created 0
# HELP demo_inflight In-flight.
# TYPE demo_inflight gauge
demo_inflight 2.5
# HELP demo_requests Total demo requests.
# TYPE demo_requests counter
demo_requests_total{method="GET"} 3
demo_requests_created{method="GET"} 0
# EOF"#,
        );
        assert!(
            body.ends_with("# EOF\n"),
            "document must terminate with # EOF"
        );
    }

    #[test]
    fn counter_family_headers_strip_total_suffix() {
        let registry = Registry::new();
        registry
            .counter("http_requests_total", "h", &[])
            .expect("with suffix");
        registry
            .counter("events", "h", &[])
            .expect("without suffix");
        let body = registry.render_as(Format::OpenMetrics);

        assert!(body.contains("# TYPE http_requests counter"), "{body}");
        assert!(body.contains("http_requests_total 0"));
        assert!(body.contains("http_requests_created "));
        // A counter registered without a `_total` suffix still exposes
        // `_total`-suffixed samples per the OpenMetrics counter convention.
        assert!(body.contains("# TYPE events counter"), "{body}");
        assert!(body.contains("events_total 0"));
        assert!(body.contains("events_created "));
    }

    #[test]
    fn utf8_names_quote_in_openmetrics_and_vanish_from_prom_text() {
        let registry = Registry::new().with_name_policy(NamePolicy::Utf8);
        registry
            .counter("service.http.requests", "Dotted name.", &[("route", "/")])
            .expect("utf8 name under Utf8 policy");

        let om = registry.render_as(Format::OpenMetrics);
        assert!(
            om.contains(r#"# HELP "service.http.requests" Dotted name."#),
            "{om}"
        );
        assert!(
            om.contains(r#"# TYPE "service.http.requests" counter"#),
            "{om}"
        );
        assert!(
            om.contains(r#"{"service.http.requests_total",route="/"} 0"#),
            "{om}"
        );
        assert!(
            om.contains(r#"{"service.http.requests_created",route="/""#),
            "{om}"
        );
        assert!(om.ends_with("# EOF\n"));

        let prom = registry.render_as(Format::PromText);
        assert!(
            !prom.contains("service.http.requests"),
            "legacy text format cannot represent UTF-8 names: {prom}"
        );
        assert_eq!(prom, registry.render());
    }

    #[test]
    fn empty_registry_renders_bare_eof() {
        let registry = Registry::new();
        assert_eq!(registry.render_as(Format::OpenMetrics), "# EOF\n");
    }

    #[test]
    fn prom_text_render_is_unchanged_by_the_feature() {
        let registry = Registry::new();
        let c = registry
            .counter("plain_total", "Plain.", &[])
            .expect("register");
        c.inc();
        let body = registry.render_as(Format::PromText);
        assert!(body.contains("# TYPE plain_total counter"));
        assert!(body.contains("plain_total 1"));
        assert!(!body.contains("# EOF"), "0.0.4 has no EOF terminator");
        assert!(!body.contains("_created"));
    }

    #[test]
    fn encoder_reports_openmetrics_content_type() {
        let encoder = Encoder::new(Format::OpenMetrics);
        assert_eq!(
            encoder.content_type(),
            "application/openmetrics-text; version=1.0.0; charset=utf-8"
        );
        let registry = Registry::new();
        assert_eq!(
            encoder.encode_to_string(&registry),
            registry.render_as(Format::OpenMetrics)
        );
    }
}
