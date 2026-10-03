//! Text exposition rendering: Prometheus 0.0.4 and OpenMetrics 1.0.0.
//!
//! Only the registry calls this; format details live here so the registry
//! stays about semantics and this file stays about bytes. Both formats
//! group each metric family under one `# HELP`/`# TYPE` header pair; they
//! differ in terminators, `_total`/`_created` naming, UTF-8 names, and
//! exemplars.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::registry::{is_legacy_name, MetricKind, Series};

/// Prometheus text 0.0.4: one `# HELP`/`# TYPE` header per family, then
/// each series' samples. Series with non-legacy (UTF-8) names cannot be
/// represented and are omitted.
pub(crate) fn render_family_prom(out: &mut String, help: &str, series: &[Arc<Series>]) {
    let Some(first) = series.first() else {
        return;
    };
    if !is_legacy_name(&first.name) {
        return;
    }
    let name = first.name.as_str();
    let _ = writeln!(out, "# HELP {name} {}", escape_help(help));
    let _ = writeln!(out, "# TYPE {name} {}", type_name(&first.kind));
    for s in series {
        render_samples_prom(out, s);
    }
}

fn render_samples_prom(out: &mut String, series: &Series) {
    let name = series.name.as_str();
    let labels = label_attrs(&series.labels);
    match &series.kind {
        MetricKind::Counter(c) => {
            let _ = writeln!(out, "{name}{labels} {}", c.snapshot());
        }
        MetricKind::Gauge(g) => {
            let _ = writeln!(out, "{name}{labels} {}", format_number(g.snapshot()));
        }
        MetricKind::Histogram(h) => render_histogram_samples_prom(out, name, &labels, h),
    }
}

/// OpenMetrics text 1.0.0: family header (counter family names carry no
/// `_total` suffix), samples with `_total`/`_created` naming, quoted
/// UTF-8 names where needed, optional exemplars, terminated by `# EOF`
/// (appended once by the caller).
#[cfg(feature = "openmetrics")]
pub(crate) fn render_family_om(out: &mut String, help: &str, series: &[Arc<Series>]) {
    let Some(first) = series.first() else {
        return;
    };
    let family: &str = match &first.kind {
        MetricKind::Counter(_) => first.name.strip_suffix("_total").unwrap_or(&first.name),
        _ => first.name.as_str(),
    };
    let header = om_metric_ref(family);
    let _ = writeln!(out, "# HELP {header} {}", escape_help(help));
    let _ = writeln!(out, "# TYPE {header} {}", type_name(&first.kind));
    for s in series {
        render_samples_om(out, s, family);
    }
}

#[cfg(feature = "openmetrics")]
fn render_samples_om(out: &mut String, series: &Series, family: &str) {
    let created = format_number(series.created);
    match &series.kind {
        MetricKind::Counter(c) => {
            let _ = write!(
                out,
                "{} {}",
                om_sample_head(&format!("{family}_total"), &series.labels),
                c.snapshot()
            );
            #[cfg(feature = "exemplars")]
            if let Some(x) = c.snapshot_exemplar() {
                write_exemplar(out, &x);
            }
            out.push('\n');
            om_sample(out, &format!("{family}_created"), &series.labels, &created);
        }
        MetricKind::Gauge(g) => {
            // Per the OpenMetrics spec, gauges MUST NOT carry a `_created`
            // series.
            om_sample(
                out,
                &series.name,
                &series.labels,
                &format_number(g.snapshot()),
            );
        }
        MetricKind::Histogram(h) => {
            for (i, bound) in h.bucket_bounds().iter().enumerate() {
                let mut le_labels = series.labels.clone();
                le_labels.push(("le".to_string(), format_number(*bound)));
                om_sample(
                    out,
                    &format!("{}_bucket", series.name),
                    &le_labels,
                    &h.cumulative_count(i).to_string(),
                );
            }
            let mut inf_labels = series.labels.clone();
            inf_labels.push(("le".to_string(), "+Inf".to_string()));
            om_sample(
                out,
                &format!("{}_bucket", series.name),
                &inf_labels,
                &h.count().to_string(),
            );
            om_sample(
                out,
                &format!("{}_sum", series.name),
                &series.labels,
                &format_number(h.sum()),
            );
            om_sample(
                out,
                &format!("{}_count", series.name),
                &series.labels,
                &h.count().to_string(),
            );
            om_sample(
                out,
                &format!("{}_created", series.name),
                &series.labels,
                &created,
            );
        }
    }
}

/// Writes one OpenMetrics sample line: head (name or quoted name, plus
/// labels) then value, terminated with LF.
#[cfg(feature = "openmetrics")]
fn om_sample(out: &mut String, name: &str, labels: &[(String, String)], value: &str) {
    let _ = writeln!(out, "{} {}", om_sample_head(name, labels), value);
}

/// The sample-line head for `name`: `name{k="v",...}` when the name is a
/// legacy identifier; the quoted OpenMetrics UTF-8 form
/// `{"name",k="v",...}` otherwise (per the spec, series suffixes such as
/// `_total` sit inside the quotes).
#[cfg(feature = "openmetrics")]
fn om_sample_head(name: &str, labels: &[(String, String)]) -> String {
    if is_legacy_name(name) {
        return format!("{name}{}", label_attrs(labels));
    }
    let mut out = String::from("{");
    write_quoted_name(&mut out, name);
    if !labels.is_empty() {
        out.push(',');
        out.push_str(&label_list(labels));
    }
    out.push('}');
    out
}

/// Appends ` # {k="v",...} value` — an OpenMetrics exemplar.
#[cfg(all(feature = "openmetrics", feature = "exemplars"))]
fn write_exemplar(out: &mut String, x: &crate::exemplar::ExemplarSnapshot) {
    out.push_str(" # {");
    for (i, (k, v)) in x.labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(k);
        out.push_str("=\"");
        escape_label_value(out, v);
        out.push('"');
    }
    let _ = write!(out, "}} {}", format_exemplar_value(x.value));
}

/// `name` or `"quoted name"` for header lines (`# HELP`/`# TYPE`).
#[cfg(feature = "openmetrics")]
fn om_metric_ref(name: &str) -> String {
    if is_legacy_name(name) {
        name.to_string()
    } else {
        let mut out = String::new();
        write_quoted_name(&mut out, name);
        out
    }
}

/// Writes `name` in OpenMetrics quoted form with `\\` and `\"` escapes.
#[cfg(feature = "openmetrics")]
fn write_quoted_name(out: &mut String, name: &str) {
    out.push('"');
    for c in name.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

/// `le`-bucket, `_sum`, and `_count` samples for the legacy text format
/// (the OpenMetrics path renders its own, including `_created`).
fn render_histogram_samples_prom(out: &mut String, name: &str, labels: &str, h: &crate::Histogram) {
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

fn type_name(kind: &MetricKind) -> &'static str {
    match kind {
        MetricKind::Counter(_) => "counter",
        MetricKind::Gauge(_) => "gauge",
        MetricKind::Histogram(_) => "histogram",
    }
}

/// Labels rendered in braces (`{k="v",...}`), or empty.
fn label_attrs(labels: &[(String, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    format!("{{{}}}", label_list(labels))
}

/// `k="v",k2="v2"` — no surrounding braces.
fn label_list(labels: &[(String, String)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(k);
        out.push_str("=\"");
        escape_label_value(&mut out, v);
        out.push('"');
    }
    out
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

fn format_number(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Exemplar values render as floats with at least one decimal (`1.0`),
/// matching the OpenMetrics exemplar grammar's float convention.
#[cfg(all(feature = "openmetrics", feature = "exemplars"))]
fn format_exemplar_value(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}
