//! Prometheus text exposition format 0.0.4 rendering.
//!
//! Only the registry calls this; format details live here so the registry
//! stays about semantics and this file stays about bytes.

use std::fmt::Write as _;

use crate::registry::{MetricKind, Series};

pub(crate) fn render_series(out: &mut String, help: &str, series: &Series) {
    let name = series.name.as_str();
    let type_name = match &series.kind {
        MetricKind::Counter(_) => "counter",
        MetricKind::Gauge(_) => "gauge",
        MetricKind::Histogram(_) => "histogram",
    };

    let _ = writeln!(out, "# HELP {name} {}", escape_help(help));
    let _ = writeln!(out, "# TYPE {name} {type_name}");

    let labels = render_labels(&series.labels);
    match &series.kind {
        MetricKind::Counter(c) => {
            let _ = writeln!(out, "{name}{labels} {}", c.snapshot());
        }
        MetricKind::Gauge(g) => {
            let _ = writeln!(out, "{name}{labels} {}", format_number(g.snapshot()));
        }
        MetricKind::Histogram(h) => {
            for (i, bound) in h.bucket_bounds().iter().enumerate() {
                let _ = writeln!(
                    out,
                    "{name}_bucket{labels}{{le=\"{}\"}} {}",
                    format_number(*bound),
                    h.cumulative_count(i)
                );
            }
            let _ = writeln!(out, "{name}_bucket{labels}{{le=\"+Inf\"}} {}", h.count());
            let _ = writeln!(out, "{name}_sum{labels} {}", format_number(h.sum()));
            let _ = writeln!(out, "{name}_count{labels} {}", h.count());
        }
    }
}

fn render_labels(labels: &[(String, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut out = String::from("{");
    for (i, (k, v)) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(k);
        out.push_str("=\"");
        escape_label_value(&mut out, v);
        out.push('"');
    }
    out.push('}');
    out
}

fn escape_label_value(out: &mut String, v: &str) {
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
}

/// Per the exposition format: in HELP lines only backslash and newline are
/// escaped; double quotes are literal.
fn escape_help(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

fn format_number(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
