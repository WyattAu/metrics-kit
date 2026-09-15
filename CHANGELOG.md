# Changelog

All notable changes to this project are documented here. Format: [Keep a
Changelog](https://keepachangelog.com/) — versions follow [semver](https://semver.org).

## [0.1.0] - 2026-09-15

### Added

- `Registry` with fallible registration: name/label validation,
  duplicate-series rejection, cardinality budget
  (`Registry::with_max_series`).
- Lock-free, cache-line-padded `Counter` (relaxed `fetch_add`), `Gauge`
  (f64 CAS loop), `Histogram` (cumulative `le` buckets + `_sum`/`_count`,
  O(1) hot path, render-time accumulation).
- Prometheus text exposition format 0.0.4 rendering with spec-correct
  escaping (HELP: `\`, `\n`; label values also `"`), deterministic
  `BTreeMap` family ordering.
- `axum` feature: `metrics_route(Arc<Registry>)` serving `GET /metrics`
  with `text/plain; version=0.0.4` content type.
- Criterion benches (`counter_inc`, `gauge_add`, `histogram_observe`,
  `registry_render_small`), concurrency integration tests, parseability
  fuzz-adjacent line checks.
