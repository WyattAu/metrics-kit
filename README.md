# metrics-kit

Lock-free Prometheus-exposition metrics kit for Rust — the shared hot-path
metrics pattern of the WyattAu estate, suitable for VictoriaMetrics, Grafana
Agent, vmagent, and any Prometheus-compatible scraper.

- **Hot path is lock-free**: relaxed `AtomicU64` ops on cache-line-padded
  cells; registration handles are `Clone` and share state.
- **Counters, gauges, histograms** with cumulative `le` buckets, `_sum`,
  `_count` per the text exposition format 0.0.4.
- **Cardinality guard**: registries carry a series budget so a runaway
  label loop cannot explode VictoriaMetrics series counts.
- **Deterministic renders**: `BTreeMap`-ordered families so scrapes diff
  cleanly; escapes match the spec (HELP: `\`, `\n`; label values also `"`).
- **Optional `axum` feature**: one-call `GET /metrics` route.
- **`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`**, clippy
  `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` denied.

## Install

```toml
[dependencies]
metrics-kit = "0.1"
```

## Example

```rust
use metrics_kit::{Counter, Gauge, Histogram, Registry};
use std::sync::Arc;

let registry = Arc::new(Registry::with_max_series(1024));
let requests: Counter = registry
    .counter("http_requests_total", "Total HTTP requests.", &[("method", "GET")])
    .expect("unique series");
let inflight: Gauge = registry
    .gauge("http_inflight", "In-flight requests.", &[])
    .expect("unique series");
let latency: Histogram = registry
    .histogram("http_request_duration_seconds", "Request duration.", &[])
    .expect("unique series");

requests.inc();
inflight.set(3.0);
latency.observe(0.013);

assert!(registry.render().contains("http_requests_total 1"));
```

Serve it:

```toml
[dependencies]
metrics-kit = { version = "0.1", features = ["axum"] }
```

```rust,ignore
let app = axum::Router::new()
    .route("/", axum::routing::get(|| async { "ok" }))
    .merge(metrics_kit::metrics_route(Arc::clone(&registry)));
```

Protect `/metrics` with network policy or auth middleware; the route is
unauthenticated by design.

## Performance

Measured with criterion on the committed bench suite
(`cargo bench`); see `benches/registry_bench.rs`:

| Operation | Path | Cost model |
|---|---|---|
| `counter.inc()` | hot | 1 relaxed `fetch_add`, own cache line |
| `gauge.add(x)` | hot | CAS loop, contended retries read-fresh |
| `histogram.observe(x)` | hot | linear bucket scan (14 cmps default) + CAS sum |
| `registry.render()` | scrape | O(series), single read lock |

P50/P99 percentile reporting for *your service's* request latency comes
from the histogram families; publish your measured numbers in your
README per the estate standard.

## Feature flags

| Feature | Default | Description |
|---|---|---|
| `axum` | no | `metrics_route(Arc<Registry>)` → `Router` with `GET /metrics` |

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT)
at your option.
