//! Stage 10.3 (D6): the metrics registry.
//!
//! A process-global registry of counters and histograms rendered in the
//! Prometheus text exposition format at `GET /metrics`. Deliberately small
//! and dependency-free: the daemon needs a handful of well-named series
//! (request latency by route, LLM dispatch latency and token spend, store
//! lock wait, hook waterfall duration, tool calls), not a metrics framework.
//!
//! Names are fixed constants so dashboards and alerts can rely on them;
//! label values are bounded by construction (route templates, provider ids,
//! hook points, tenant ids) — never raw paths or user text.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use parking_lot::Mutex;

pub const HTTP_REQUESTS_TOTAL: &str = "kura_http_requests_total";
pub const HTTP_REQUEST_DURATION_SECONDS: &str = "kura_http_request_duration_seconds";
pub const LLM_DISPATCH_DURATION_SECONDS: &str = "kura_llm_dispatch_duration_seconds";
pub const LLM_DISPATCHES_TOTAL: &str = "kura_llm_dispatches_total";
pub const LLM_TOKENS_TOTAL: &str = "kura_llm_tokens_total";
pub const STORE_LOCK_WAIT_SECONDS: &str = "kura_store_lock_wait_seconds";
pub const HOOK_DURATION_SECONDS: &str = "kura_hook_duration_seconds";
pub const CHAT_TOOL_CALLS_TOTAL: &str = "kura_chat_tool_calls_total";
pub const CHAT_TOOL_CALL_DURATION_SECONDS: &str = "kura_chat_tool_call_duration_seconds";

/// Help text per series, rendered as `# HELP`.
const HELP: &[(&str, &str)] = &[
    (
        HTTP_REQUESTS_TOTAL,
        "HTTP requests by route template, method and status class.",
    ),
    (
        HTTP_REQUEST_DURATION_SECONDS,
        "HTTP request latency by route template.",
    ),
    (
        LLM_DISPATCH_DURATION_SECONDS,
        "LLM dispatch latency by provider and outcome.",
    ),
    (
        LLM_DISPATCHES_TOTAL,
        "LLM dispatches by provider and outcome.",
    ),
    (
        LLM_TOKENS_TOTAL,
        "LLM tokens by tenant, provider and kind (input|output).",
    ),
    (
        STORE_LOCK_WAIT_SECONDS,
        "Time spent waiting for a store connection, by role (writer|reader).",
    ),
    (
        HOOK_DURATION_SECONDS,
        "Plugin hook waterfall duration by hook point.",
    ),
    (
        CHAT_TOOL_CALLS_TOTAL,
        "Chat tool calls by tool name and outcome.",
    ),
    (
        CHAT_TOOL_CALL_DURATION_SECONDS,
        "Chat tool call duration by tool name.",
    ),
];

/// Histogram bucket upper bounds, in seconds. One shared ladder keeps the
/// exposition predictable; nothing here is faster than 1ms or slower than
/// a minute in a way a dashboard needs to resolve.
pub const BUCKETS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

type Labels = BTreeMap<String, String>;

#[derive(Default, Clone)]
struct HistogramSeries {
    counts: Vec<u64>,
    sum: f64,
    count: u64,
}

#[derive(Default)]
struct Inner {
    counters: BTreeMap<(String, Labels), u64>,
    histograms: BTreeMap<(String, Labels), HistogramSeries>,
}

/// The registry. Obtain the process-global one with [`registry`]; tests
/// may build their own with [`Registry::new`].
#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
}

fn labels_of(pairs: &[(&str, &str)]) -> Labels {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Registry::default()
    }

    pub fn inc(&self, name: &str, labels: &[(&str, &str)]) {
        self.add(name, labels, 1);
    }

    pub fn add(&self, name: &str, labels: &[(&str, &str)], by: u64) {
        let mut inner = self.inner.lock();
        *inner
            .counters
            .entry((name.to_string(), labels_of(labels)))
            .or_insert(0) += by;
    }

    pub fn observe(&self, name: &str, labels: &[(&str, &str)], value_seconds: f64) {
        let mut inner = self.inner.lock();
        let series = inner
            .histograms
            .entry((name.to_string(), labels_of(labels)))
            .or_insert_with(|| HistogramSeries {
                counts: vec![0; BUCKETS.len()],
                ..HistogramSeries::default()
            });
        for (i, bound) in BUCKETS.iter().enumerate() {
            if value_seconds <= *bound {
                series.counts[i] += 1;
            }
        }
        series.sum += value_seconds;
        series.count += 1;
    }

    /// Current value of one counter series (tests and self-checks).
    #[must_use]
    pub fn counter_value(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        self.inner
            .lock()
            .counters
            .get(&(name.to_string(), labels_of(labels)))
            .copied()
            .unwrap_or(0)
    }

    /// Observation count of one histogram series.
    #[must_use]
    pub fn histogram_count(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        self.inner
            .lock()
            .histograms
            .get(&(name.to_string(), labels_of(labels)))
            .map(|s| s.count)
            .unwrap_or(0)
    }

    /// Renders the Prometheus text exposition (version 0.0.4).
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        let inner = self.inner.lock();
        let mut out = String::new();
        let mut names: Vec<&str> = inner
            .counters
            .keys()
            .map(|(n, _)| n.as_str())
            .chain(inner.histograms.keys().map(|(n, _)| n.as_str()))
            .collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            if let Some((_, help)) = HELP.iter().find(|(n, _)| *n == name) {
                out.push_str(&format!("# HELP {name} {help}\n"));
            }
            let is_histogram = inner.histograms.keys().any(|(n, _)| n == name);
            out.push_str(&format!(
                "# TYPE {name} {}\n",
                if is_histogram { "histogram" } else { "counter" }
            ));
            for ((n, labels), value) in &inner.counters {
                if n == name {
                    out.push_str(&format!("{name}{} {value}\n", render_labels(labels, None)));
                }
            }
            for ((n, labels), series) in &inner.histograms {
                if n != name {
                    continue;
                }
                for (i, bound) in BUCKETS.iter().enumerate() {
                    out.push_str(&format!(
                        "{name}_bucket{} {}\n",
                        render_labels(labels, Some(&format_bound(*bound))),
                        series.counts[i]
                    ));
                }
                out.push_str(&format!(
                    "{name}_bucket{} {}\n",
                    render_labels(labels, Some("+Inf")),
                    series.count
                ));
                out.push_str(&format!(
                    "{name}_sum{} {}\n",
                    render_labels(labels, None),
                    series.sum
                ));
                out.push_str(&format!(
                    "{name}_count{} {}\n",
                    render_labels(labels, None),
                    series.count
                ));
            }
        }
        out
    }
}

fn format_bound(bound: f64) -> String {
    let s = format!("{bound}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

fn render_labels(labels: &Labels, le: Option<&str>) -> String {
    let mut parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", escape(v)))
        .collect();
    if let Some(le) = le {
        parts.push(format!("le=\"{le}\""));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", parts.join(","))
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

static GLOBAL: OnceLock<Registry> = OnceLock::new();

/// The process-global registry every instrumented crate records into.
pub fn registry() -> &'static Registry {
    GLOBAL.get_or_init(Registry::new)
}

/// Status class label (`2xx`, `4xx`, …) so status is a bounded label.
#[must_use]
pub fn status_class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_histograms_render_in_exposition_format() {
        let r = Registry::new();
        r.inc(
            HTTP_REQUESTS_TOTAL,
            &[
                ("route", "/v1/chat/query"),
                ("method", "POST"),
                ("status", "2xx"),
            ],
        );
        r.inc(
            HTTP_REQUESTS_TOTAL,
            &[
                ("route", "/v1/chat/query"),
                ("method", "POST"),
                ("status", "2xx"),
            ],
        );
        r.observe(
            HTTP_REQUEST_DURATION_SECONDS,
            &[("route", "/v1/chat/query")],
            0.012,
        );
        r.observe(
            HTTP_REQUEST_DURATION_SECONDS,
            &[("route", "/v1/chat/query")],
            3.0,
        );
        let text = r.render_prometheus();
        assert!(text.contains("# TYPE kura_http_requests_total counter\n"));
        assert!(text.contains("kura_http_requests_total{method=\"POST\",route=\"/v1/chat/query\",status=\"2xx\"} 2\n"), "{text}");
        assert!(text.contains("# TYPE kura_http_request_duration_seconds histogram\n"));
        assert!(text.contains("kura_http_request_duration_seconds_bucket{route=\"/v1/chat/query\",le=\"0.025\"} 1\n"), "{text}");
        assert!(text.contains(
            "kura_http_request_duration_seconds_bucket{route=\"/v1/chat/query\",le=\"+Inf\"} 2\n"
        ));
        assert!(
            text.contains("kura_http_request_duration_seconds_count{route=\"/v1/chat/query\"} 2\n")
        );
        assert_eq!(
            r.counter_value(
                HTTP_REQUESTS_TOTAL,
                &[
                    ("route", "/v1/chat/query"),
                    ("method", "POST"),
                    ("status", "2xx")
                ]
            ),
            2
        );
        assert_eq!(
            r.histogram_count(
                HTTP_REQUEST_DURATION_SECONDS,
                &[("route", "/v1/chat/query")]
            ),
            2
        );
    }

    #[test]
    fn label_values_are_escaped() {
        let r = Registry::new();
        r.inc(CHAT_TOOL_CALLS_TOTAL, &[("name", "a\"b\\c")]);
        assert!(r.render_prometheus().contains("name=\"a\\\"b\\\\c\""));
    }

    #[test]
    fn status_classes_are_bounded() {
        assert_eq!(status_class(200), "2xx");
        assert_eq!(status_class(404), "4xx");
        assert_eq!(status_class(503), "5xx");
    }
}
