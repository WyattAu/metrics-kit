# metrics-kit

Lock-free Prometheus/OpenMetrics-exposition metrics kit for Rust — the
shared hot-path metrics pattern of the WyattAu estate, suitable for
VictoriaMetrics, Grafana Agent, vmagent, and any Prometheus-compatible
scraper.

- **Hot path is lock-free**: relaxed `AtomicU64` ops on cache-line-padded
  cells; registration handles are `Clone` and share state.
- **Counters, gauges, histograms** with cumulative `le` buckets, `_sum`,
  `_count` per the text exposition format 0.0.4.
- **OpenMetrics 1.0.0** (`openmetrics` feature): `# EOF` terminator,
  `_created` series, counter `_total` family-header naming, quoted UTF-8
  metric names, and `Accept`-header negotiation with
  `application/openmetrics-text; version=1.0.0`.
- **Exemplars** (`exemplars` feature): lock-free, allocation-free
  last-writer-wins exemplar slot on `Counter`, rendered as
  `# {trace_id="..."} 1.0` in OpenMetrics mode.
- **Cardinality guard**: registries carry a series budget so a runaway
  label loop cannot explode VictoriaMetrics series counts.
- **Deterministic renders**: `BTreeMap`-ordered families so scrapes diff
  cleanly; escapes match the spec (HELP: `\`, `\n`; label values also `"`).
- **Optional `axum` feature**: one-call `GET /metrics` route with format
  negotiation.
- **`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`**, clippy
  `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` denied.

## Install

```toml
[dependencies]
metrics-kit = "0.2"
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
metrics-kit = { version = "0.2", features = ["axum"] }
```

```rust,ignore
let app = axum::Router::new()
    .route("/", axum::routing::get(|| async { "ok" }))
    .merge(metrics_kit::metrics_route(Arc::clone(&registry)));
```

Protect `/metrics` with network policy or auth middleware; the route is
unauthenticated by design.

## OpenMetrics and format negotiation

With the `openmetrics` feature, the same registry renders OpenMetrics
text 1.0.0 — `# EOF`-terminated, with `_created` series for counters and
histograms (gauges are excluded per the spec) and counter family headers
under the `_total`-stripped name:

```rust,ignore
use metrics_kit::{Encoder, Format};

let encoder = Encoder::new(Format::OpenMetrics);
let (body, content_type) =
    (encoder.encode_to_string(&registry), encoder.content_type());
// content_type = "application/openmetrics-text; version=1.0.0; charset=utf-8"
```

The `axum` route negotiates automatically: a scrape sending
`Accept: application/openmetrics-text` gets OpenMetrics, everything else
gets 0.0.4 text — each with the correct `Content-Type`.

UTF-8 metric names (e.g. `service.http.requests`) are opt-in per
registry and render in the spec's quoted form:

```rust,ignore
use metrics_kit::NamePolicy;

let registry = metrics_kit::Registry::new().with_name_policy(NamePolicy::Utf8);
registry.counter("service.http.requests", "Dotted name.", &[])?;
// OpenMetrics: {"service.http.requests_total",route="/"} 42
```

Legacy 0.0.4 renders omit UTF-8-named series (the legacy format cannot
represent them).

## Exemplars

With the `exemplars` feature, a counter carries a fixed-capacity
exemplar slot — a cache-line-padded, lock-free seqlock cell (the estate's
percentile-ring pattern). Recording is allocation-free; the newest
exemplar wins:

```rust,ignore
requests.with_exemplar(&[("span", "root")], 1, "trace_id", "7b3f");
```

OpenMetrics scrape line:

```text
http_requests_total 1 # {trace_id="7b3f",span="root"} 1.0
```

Exemplars never appear in 0.0.4 output (the legacy format has no
exemplar syntax).

## Histogram bucket constructors

Prometheus-client-parity ladder builders feed
`histogram_with_buckets`:

```rust,ignore
let exp = metrics_kit::exponential_buckets(0.001, 10.0, 4)?; // 1ms → 1s
let lin = metrics_kit::linear_buckets(0.0, 100.0, 5)?;       // 0 → 400
let latency = registry
    .histogram_with_buckets("request_duration_seconds", "Duration.", &[], exp)?;
```

## Summaries and quantiles (honest scope)

There is **no summary/quantile sketch (DDSketch et al.) in v0.2**, by
design: histogram `le` buckets plus the estate's `percentile-kit` cover
percentile estimation, and a wrong sketch silently poisons every SLO
built on it. If a sketch lands later it ships behind its own feature
with published error bounds.

## Process-global registry

Application wiring that cannot thread a handle everywhere can use the
init-once process-global registry (the telemetry-init pattern):

```rust,ignore
let requests = metrics_kit::Registry::global()
    .counter("job_ticks_total", "Ticks.", &[])?;
```

## Performance

Measured with criterion on the committed bench suite
(`cargo bench`); see `benches/registry_bench.rs`:

| Operation | Path | Cost model |
|---|---|---|
| `counter.inc()` | hot | 1 relaxed `fetch_add`, own cache line |
| `counter.with_exemplar(...)` | hot | seqlock store into a fixed slot, no alloc |
| `gauge.add(x)` | hot | CAS loop, contended retries read-fresh |
| `histogram.observe(x)` | hot | linear bucket scan (14 cmps default) + CAS sum |
| `registry.render()` / `render_as` | scrape | O(series), single read lock |

P50/P99 percentile reporting for *your service's* request latency comes
from the histogram families; publish your measured numbers in your
README per the estate standard.

## Feature flags

| Feature | Default | Description |
|---|---|---|
| `axum` | no | `metrics_route(Arc<Registry>)` → `Router` with `GET /metrics` (Accept negotiation with `openmetrics`) |
| `openmetrics` | no | OpenMetrics 1.0.0 rendering: `Format::OpenMetrics`, `render_as`, `_created`, quoted UTF-8 names, `# EOF` |
| `exemplars` | no | `Counter::with_exemplar` / `clear_exemplar` — lock-free last-writer-wins exemplar slot |

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT)
at your option.
