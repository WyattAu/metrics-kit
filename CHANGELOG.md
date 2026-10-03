# Changelog

All notable changes to this project are documented here. Format: [Keep a
Changelog](https://keepachangelog.com/) — versions follow [semver](https://semver.org).

## [0.2.0] - 2026-10-03

### Added

- **`openmetrics` feature** — OpenMetrics text format 1.0.0 rendering:
  `Registry::render_as(Format::OpenMetrics)` terminates with `# EOF`,
  emits spec-correct `_created` series (counters and histograms;
  gauges are excluded per the spec's MUST NOT), strips the `_total`
  suffix on counter `# HELP`/`# TYPE` family headers, renders UTF-8
  metric names in the quoted `{"name",label="v"}` form, and serves
  `application/openmetrics-text; version=1.0.0; charset=utf-8` as the
  content type. `Format` (`PromText` | `OpenMetrics`) +
  `Format::content_type()` back the scrape response so scrapers
  negotiate via `Accept`; the `axum` `/metrics` route negotiates
  automatically.
- **`NamePolicy`** (`Registry::with_name_policy`) — opt-in UTF-8 metric
  names (`NamePolicy::Utf8`): any non-empty, control-character-free name
  ≤255 bytes. Quoted in OpenMetrics renders; omitted from legacy
  Prometheus-text renders, which cannot represent them. Label names stay
  legacy-validated in both policies.
- **`exemplars` feature** — `Counter::with_exemplar(labels, value,
  exemplar_key, exemplar_val)` plus `Counter::clear_exemplar`: a fixed
  -capacity (4 pairs × 40 bytes), lock-free, allocation-free
  last-writer-wins exemplar slot (seqlock protocol, cache-line padded —
  same pattern as the estate's percentile ring). Rendered in OpenMetrics
  mode as `# {trace_id="..."} 1.0` after the sample; never rendered in
  0.0.4 output. Constants `MAX_EXEMPLAR_EXTRA_PAIRS` / `MAX_EXEMPLAR_STR_BYTES`.
- **Prometheus-client bucket constructors** — `exponential_buckets(start,
  factor, count)` and `linear_buckets(start, width, count)` with
  prometheus/prometheus-client semantics and typed `InvalidBuckets`
  errors, feeding `Registry::histogram_with_buckets`.
- **`Registry::global()`** — process-global registry via `OnceLock`,
  init-once under concurrency (the telemetry-init pattern).
- **`Encoder`** — `Encoder::new(format)` bundling format selection,
  `content_type()`, `encode(&Registry, &mut String)`, and
  `encode_to_string(&Registry)`.

### Changed

- Renders emit one `# HELP`/`# TYPE` header pair per metric family
  (previously one per series); sample lines and ordering are unchanged.
- `metrics_route` reads the `Accept` header when the `openmetrics`
  feature is on and serves the negotiated format with the matching
  content type.

### Deferred

- **Summary / quantile sketch (DDSketch et al.)** — deliberately not in
  v0.2. Histogram `le` buckets plus `percentile-kit` cover estimation;
  a wrong sketch silently poisons every SLO built on it. Any future
  sketch ships behind its own feature with published error bounds.

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
