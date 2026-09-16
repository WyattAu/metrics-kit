#![no_main]

use libfuzzer_sys::fuzz_target;
use metrics_kit::Registry;

/// Split `data` into up to `n` length-prefixed records (u16 LE length +
/// payload). Missing records come back empty; trailing bytes are ignored.
fn split_records(mut data: &[u8], n: usize) -> Vec<&[u8]> {
    let mut parts = Vec::with_capacity(n);
    while parts.len() < n && data.len() >= 2 {
        let len = u16::from_le_bytes([data[0], data[1]]) as usize;
        let rest = &data[2..];
        let take = len.min(rest.len());
        parts.push(&rest[..take]);
        data = &rest[take..];
    }
    while parts.len() < n {
        parts.push(b"");
    }
    parts
}

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(4096)];
    let parts = split_records(data, 2);
    let name = String::from_utf8_lossy(parts[0]);

    // Gauge values from raw f64 bits: NaN, infinities, -0.0, subnormals.
    // `format_number` must render all of them without panicking.
    let gauge_bits = parts[1].first_chunk::<8>().copied().unwrap_or([0; 8]);
    let gauge_value = f64::from_le_bytes(gauge_bits);
    let observe_bits = parts[1]
        .get(8..16)
        .and_then(|b| b.first_chunk::<8>().copied())
        .unwrap_or([0xff; 8]);
    let observe_value = f64::from_le_bytes(observe_bits);

    // Bucket bounds from raw f64 bits: empty, non-ascending, NaN members —
    // registration must reject with InvalidBuckets, never panic.
    let mut buckets = Vec::new();
    for chunk in parts[1].get(16..).unwrap_or(b"").chunks_exact(8) {
        buckets.push(f64::from_le_bytes(chunk.try_into().unwrap_or([0; 8])));
    }

    let registry = Registry::new();
    let rendered = match registry.histogram_with_buckets(&name, "fuzz", &[], buckets) {
        Ok(hist) => {
            hist.observe(observe_value);
            hist.observe(gauge_value);
            true
        }
        Err(_) => true, // Err is the contract; panic is the bug
    };
    if let Ok(gauge) = registry.gauge(&name, "fuzz", &[]) {
        gauge.set(gauge_value);
        gauge.add(observe_value);
    }
    let _ = registry.render();
    let _ = rendered;
});
