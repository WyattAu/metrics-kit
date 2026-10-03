//! Exposition encoders: wire-format selection and content types.
//!
//! Scrapers negotiate formats via the `Accept` header; the registry renders
//! whichever [`Format`] was agreed on. [`Encoder`] bundles the format with
//! its [`Encoder::content_type`] so the HTTP layer stays trivial.

use crate::registry::Registry;

/// Exposition wire format.
///
/// `PromText` is the Prometheus text exposition format 0.0.4 (the default,
/// universally scrapeable). `OpenMetrics` is the OpenMetrics text format
/// 1.0.0: `# EOF` terminator, `_created` series, quoted UTF-8 metric names,
/// and exemplars when the `exemplars` feature is on. Both are served from
/// the same registry; series registered with
/// [`NamePolicy::Utf8`](crate::NamePolicy::Utf8) names only appear in
/// `OpenMetrics` renders (the legacy text format cannot represent them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Format {
    /// Prometheus text exposition format 0.0.4.
    #[default]
    PromText,
    /// OpenMetrics text format 1.0.0. Requires the `openmetrics` feature.
    ///
    /// ```
    /// use metrics_kit::{Encoder, Format, Registry};
    ///
    /// let registry = Registry::new();
    /// registry
    ///     .counter("demo_requests_total", "Total demo requests", &[])
    ///     .expect("unique name");
    ///
    /// let encoder = Encoder::new(Format::OpenMetrics);
    /// let body = encoder.encode_to_string(&registry);
    /// assert!(body.contains("demo_requests_total 0"));
    /// assert!(body.ends_with("# EOF\n"));
    /// assert_eq!(
    ///     encoder.content_type(),
    ///     "application/openmetrics-text; version=1.0.0; charset=utf-8"
    /// );
    /// ```
    #[cfg(feature = "openmetrics")]
    OpenMetrics,
}

impl Format {
    /// The HTTP `Content-Type` value for a response body in this format,
    /// including the format version scrapers key on.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::PromText => "text/plain; version=0.0.4; charset=utf-8",
            #[cfg(feature = "openmetrics")]
            Self::OpenMetrics => "application/openmetrics-text; version=1.0.0; charset=utf-8",
        }
    }
}

/// Renders a [`Registry`] into an exposition format.
///
/// Cheap to copy; construct per scrape (or once at startup) and pair with
/// the format's [`Encoder::content_type`] in the response headers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Encoder {
    format: Format,
}

impl Encoder {
    /// Creates an encoder for `format`.
    pub fn new(format: Format) -> Self {
        Self { format }
    }

    /// The format this encoder emits.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The HTTP `Content-Type` value for bodies this encoder produces.
    pub fn content_type(&self) -> &'static str {
        self.format.content_type()
    }

    /// Renders the registry, appending to `out` (so a scrape handler can
    /// reuse a buffer across scrapes).
    pub fn encode(&self, registry: &Registry, out: &mut String) {
        out.push_str(&registry.render_as(self.format));
    }

    /// Renders the registry into a fresh `String`.
    pub fn encode_to_string(&self, registry: &Registry) -> String {
        registry.render_as(self.format)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn content_types_carry_format_versions() {
        assert_eq!(
            Format::PromText.content_type(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let encoder = Encoder::new(Format::PromText);
        assert_eq!(encoder.format(), Format::PromText);
        assert_eq!(encoder.content_type(), Format::PromText.content_type());
    }

    #[cfg(feature = "openmetrics")]
    #[test]
    fn openmetrics_content_type_is_1_0_0() {
        assert_eq!(
            Format::OpenMetrics.content_type(),
            "application/openmetrics-text; version=1.0.0; charset=utf-8"
        );
    }

    #[test]
    fn encoder_encodes_to_string() {
        let registry = Registry::new();
        registry
            .counter("enc_total", "Encoder.", &[])
            .expect("register");
        let text = Encoder::new(Format::PromText).encode_to_string(&registry);
        assert!(text.contains("enc_total 0"));
        let mut buf = String::new();
        Encoder::new(Format::PromText).encode(&registry, &mut buf);
        assert_eq!(buf, text);
    }
}
