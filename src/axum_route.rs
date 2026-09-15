//! Ready-made `/metrics` route for axum services.

use std::sync::Arc;

use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::Registry;

/// Adds a `GET /metrics` route returning the registry in Prometheus text
/// exposition format with the correct content type, ready for vmagent /
/// VictoriaMetrics / Grafana Agent scraping.
///
/// The registry is shared by `Arc`; pass the same handle you registered
/// metrics on. Protect the route at the router level (network policy or
/// auth middleware); this handler is unauthenticated by design.
pub fn metrics_route(registry: Arc<Registry>) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let registry = Arc::clone(&registry);
            async move {
                let body = registry.render();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(
                        header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )
                    .body(body)
                    .unwrap_or_else(|_| {
                        // Response::builder only fails on invalid status or
                        // header values, both fixed constants here.
                        Response::new(String::new())
                    })
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::Arc as StdArc;

    #[tokio::test]
    async fn serves_exposition_with_content_type() {
        let registry = StdArc::new(Registry::new());
        let counter = registry
            .counter("route_test_total", "Route test.", &[])
            .expect("register");
        counter.inc();

        let app = metrics_route(Arc::clone(&registry));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read");
        let text = String::from_utf8(buf).expect("utf8");

        assert!(text.contains("200 OK"));
        assert!(text.contains("text/plain; version=0.0.4"));
        assert!(text.contains("route_test_total 1"));
        server.abort();
    }
}
