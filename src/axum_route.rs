//! Ready-made `/metrics` route for axum services.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::Registry;

/// Adds a `GET /metrics` route returning the registry in the negotiated
/// exposition format, ready for vmagent / VictoriaMetrics / Grafana Agent
/// scraping. With the `openmetrics` feature, a request whose `Accept`
/// header names `application/openmetrics-text` gets OpenMetrics 1.0.0
/// (`# EOF` terminator, `_created` series, exemplars when enabled); every
/// other request gets Prometheus text 0.0.4. The response `Content-Type`
/// always states the format actually served.
///
/// The registry is shared by `Arc`; pass the same handle you registered
/// metrics on. Protect the route at the router level (network policy or
/// auth middleware); this handler is unauthenticated by design.
#[cfg(feature = "openmetrics")]
pub fn metrics_route(registry: Arc<Registry>) -> Router {
    use axum::http::HeaderMap;

    use crate::encoder::Format;

    Router::new().route(
        "/metrics",
        get(move |headers: HeaderMap| {
            let registry = Arc::clone(&registry);
            async move {
                let format = if accepts_openmetrics(&headers) {
                    Format::OpenMetrics
                } else {
                    Format::PromText
                };
                respond(format.content_type(), registry.render_as(format))
            }
        }),
    )
}

/// Whether the scraper's `Accept` header negotiates OpenMetrics.
#[cfg(feature = "openmetrics")]
fn accepts_openmetrics(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/openmetrics-text"))
}

/// Adds a `GET /metrics` route returning the registry in Prometheus text
/// exposition format 0.0.4 with the correct content type (the
/// `openmetrics` feature upgrades this route to Accept-header
/// negotiation).
///
/// The registry is shared by `Arc`; pass the same handle you registered
/// metrics on. Protect the route at the router level (network policy or
/// auth middleware); this handler is unauthenticated by design.
#[cfg(not(feature = "openmetrics"))]
pub fn metrics_route(registry: Arc<Registry>) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let registry = Arc::clone(&registry);
            async move {
                respond(
                    "text/plain; version=0.0.4; charset=utf-8",
                    registry.render(),
                )
            }
        }),
    )
}

fn respond(content_type: &'static str, body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            // Response::builder only fails on invalid status or
            // header values, both fixed constants here.
            Response::new(Body::empty())
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::Arc as StdArc;

    async fn scrape(app: Router, accept: Option<&str>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let accept_line = accept
            .map(|a| format!("Accept: {a}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "GET /metrics HTTP/1.1\r\nHost: localhost\r\n{accept_line}Connection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read");
        server.abort();
        String::from_utf8(buf).expect("utf8")
    }

    #[tokio::test]
    async fn serves_exposition_with_content_type() {
        let registry = StdArc::new(Registry::new());
        let counter = registry
            .counter("route_test_total", "Route test.", &[])
            .expect("register");
        counter.inc();

        let app = metrics_route(Arc::clone(&registry));
        let text = scrape(app, None).await;

        assert!(text.contains("200 OK"));
        assert!(text.contains("text/plain; version=0.0.4"));
        assert!(text.contains("route_test_total 1"));
    }

    #[cfg(feature = "openmetrics")]
    #[tokio::test]
    async fn negotiates_openmetrics_on_accept() {
        let registry = StdArc::new(Registry::new());
        registry
            .counter("route_om_total", "Route OM test.", &[])
            .expect("register");

        let app = metrics_route(Arc::clone(&registry));
        let text = scrape(app, Some("application/openmetrics-text; version=1.0.0")).await;

        assert!(text.contains("200 OK"));
        assert!(text.contains("application/openmetrics-text; version=1.0.0"));
        assert!(text.contains("route_om_total 0"));
        // Body terminates with the OpenMetrics EOF marker (allowing the
        // HTTP chunked/keep-alive framing between it and the socket close).
        assert!(text.contains("\n# EOF\n"));
    }

    #[cfg(feature = "openmetrics")]
    #[tokio::test]
    async fn legacy_accept_still_gets_prom_text() {
        let registry = StdArc::new(Registry::new());
        registry
            .counter("route_prom_total", "Route prom test.", &[])
            .expect("register");

        let app = metrics_route(Arc::clone(&registry));
        let text = scrape(app, Some("*/*")).await;

        assert!(text.contains("text/plain; version=0.0.4"));
        assert!(!text.contains("openmetrics"));
    }
}
