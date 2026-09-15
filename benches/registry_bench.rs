//! Criterion benches: hot-path recording and scrape rendering.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use criterion::{criterion_group, criterion_main, Criterion};
use metrics_kit::{Counter, Gauge, Histogram, Registry};
use std::hint::black_box;
use std::sync::Arc;

fn bench_counter_inc(c: &mut Criterion) {
    let registry = Registry::new();
    let counter: Counter = registry
        .counter("bench_total", "Bench.", &[])
        .expect("register");
    c.bench_function("counter_inc", |b| b.iter(|| black_box(&counter).inc()));
}

fn bench_gauge_add(c: &mut Criterion) {
    let registry = Registry::new();
    let gauge: Gauge = registry
        .gauge("bench_gauge", "Bench.", &[])
        .expect("register");
    c.bench_function("gauge_add", |b| {
        b.iter(|| black_box(&gauge).add(black_box(1.0)))
    });
}

fn bench_histogram_observe(c: &mut Criterion) {
    let registry = Registry::new();
    let hist: Histogram = registry
        .histogram("bench_duration_seconds", "Bench.", &[])
        .expect("register");
    c.bench_function("histogram_observe", |b| {
        b.iter(|| black_box(&hist).observe(black_box(0.013)))
    });
}

fn bench_registry_render(c: &mut Criterion) {
    let registry = Arc::new(Registry::new());
    let counter: Counter = registry
        .counter("render_requests_total", "Bench.", &[("method", "GET")])
        .expect("register");
    counter.inc();
    c.bench_function("registry_render_small", |b| {
        let registry = Arc::clone(&registry);
        b.iter(|| black_box(registry.render()))
    });
}

criterion_group!(
    benches,
    bench_counter_inc,
    bench_gauge_add,
    bench_histogram_observe,
    bench_registry_render
);
criterion_main!(benches);
