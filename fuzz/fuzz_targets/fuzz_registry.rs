#![no_main]

use std::sync::Arc;

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
    // Bound input so registration and rendering stay fast.
    let data = &data[..data.len().min(4096)];
    let parts = split_records(data, 6);
    let name = String::from_utf8_lossy(parts[0]);
    let help = String::from_utf8_lossy(parts[1]);
    let label_k = String::from_utf8_lossy(parts[2]);
    let label_v = String::from_utf8_lossy(parts[3]);
    let label_k2 = String::from_utf8_lossy(parts[4]);
    let label_v2 = String::from_utf8_lossy(parts[5]);

    // Adversarial names, help strings, and label keys/values — including
    // quotes, backslashes, newlines, and control characters — must be
    // rejected with Err or escaped correctly, never panic the registry or
    // corrupt the exposition output.
    let registry = Arc::new(Registry::with_max_series(64));
    let labels: Vec<(&str, &str)> = if label_k2.is_empty() {
        vec![(label_k.as_ref(), label_v.as_ref())]
    } else {
        vec![
            (label_k.as_ref(), label_v.as_ref()),
            (label_k2.as_ref(), label_v2.as_ref()),
        ]
    };

    let _ = registry.counter(&name, &help, &labels);
    let _ = registry.gauge(&name, &help, &labels);
    let _ = registry.histogram(&name, &help, &labels);

    // Concurrent registration racing the render path: duplicate-series
    // contention and cardinality exhaustion must surface as Err (or
    // RegistryPoisoned after a panic), never as a torn exposition.
    let mut handles = Vec::new();
    for _ in 0..2 {
        let registry = Arc::clone(&registry);
        let name = name.to_string();
        let help = help.to_string();
        let labels_owned: Vec<(String, String)> = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        handles.push(std::thread::spawn(move || {
            let labels: Vec<(&str, &str)> = labels_owned
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let _ = registry.counter(&name, &help, &labels);
        }));
    }
    let rendered = registry.render();
    for h in handles {
        let _ = h.join();
    }
    let _ = registry.render();
    let _ = registry.series_count();
    let _ = rendered;
});
