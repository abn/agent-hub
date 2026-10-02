//! The in-process metrics registry.
//!
//! A few counters the operator can scrape are cheap to keep and expensive to
//! reconstruct after the fact: how many requests the hub answered by method and
//! status class, how many events each project feed gained by kind, and how many
//! times an agent called each MCP tool. They are atomics bumped on the path that
//! already does the work: the HTTP layer, the event append, and the tool call.
//!
//! The registry is a crate-level [`OnceLock`], not a field on `AppState`. An
//! event append funnels through `store::events::append_in_tx_capped`, which has
//! no `AppState` in scope and is called from several modules, so a global is
//! what lets the three writers reach it without threading state or changing
//! `AppState`'s `Clone`. Nothing here is per-request allocation: every bump is
//! one relaxed atomic add, and every label is drawn from a closed set the hub
//! defines, so a scraped series name cannot grow with caller input.
//!
//! `/metrics` renders this as Prometheus text.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::events::KINDS;

/// The fixed HTTP methods the counter distinguishes. Everything else, including
/// a method this build does not know, lands in the `other` slot.
const METHODS: [&str; 8] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "OTHER",
];

/// Status classes 1xx through 5xx. The index is the class, so slot 0 is unused
/// and a class outside 1 to 5 is not a series that can appear.
const STATUS_CLASSES: usize = 6;

/// The number of event kinds the counter distinguishes, matching
/// [`KINDS`]. A kind the store does not know never reaches the append, and a
/// kind that somehow did is ignored rather than minting an unbounded label.
const EVENT_KINDS: usize = 8;

/// The name an unknown tool is counted under.
///
/// A caller supplies the tool name, so counting it verbatim would let one
/// client mint unlimited series. The hook passes this sentinel for a name the
/// router does not know; the real name is counted only when it is a tool the
/// hub actually advertises.
pub const UNKNOWN_TOOL: &str = "<unknown>";

/// A bounded ceiling on the tool labels, so a bug in the caller cannot grow the
/// map without limit. Far above the hub's tool count.
const TOOL_LABELS_MAX: usize = 256;

/// A value a tool has no ordinary name for.
const OTHER_TOOL: &str = "<other>";

/// The counters, reached through [`global`].
pub struct Metrics {
    /// `[method][status class]`, indexed by [`METHODS`] and the class number.
    http: [[AtomicU64; STATUS_CLASSES]; METHODS.len()],
    /// One slot per kind in [`KINDS`], same order.
    events: [AtomicU64; EVENT_KINDS],
    /// Tool name to count. Bounded by [`TOOL_LABELS_MAX`].
    tools: Mutex<HashMap<String, u64>>,
}

impl Metrics {
    /// A registry with every counter at zero.
    pub fn new() -> Self {
        Self {
            http: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            events: std::array::from_fn(|_| AtomicU64::new(0)),
            tools: Mutex::new(HashMap::new()),
        }
    }

    /// Count one HTTP request by method and status class (1 through 5).
    pub fn record_http(&self, method: &str, status_class: u16) {
        let m = method_slot(method);
        let class = status_class as usize;
        if class == 0 || class >= STATUS_CLASSES {
            return;
        }
        self.http[m][class].fetch_add(1, Ordering::Relaxed);
    }

    /// Count one appended event by kind. A kind outside the store's closed set
    /// is ignored: the append validates the kind before it counts.
    pub fn record_event(&self, kind: &str) {
        if let Some(slot) = KINDS.iter().position(|candidate| *candidate == kind) {
            self.events[slot].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Count one MCP tool call. The caller passes a name the router knows, or
    /// [`UNKNOWN_TOOL`]; the map's ceiling is a backstop, not a routing rule.
    pub fn record_tool(&self, tool: &str) {
        let mut tools = self.tools.lock().unwrap();
        if tools.len() >= TOOL_LABELS_MAX && !tools.contains_key(tool) {
            *tools.entry(OTHER_TOOL.to_string()).or_insert(0) += 1;
            return;
        }
        *tools.entry(tool.to_string()).or_insert(0) += 1;
    }

    /// Render every counter as Prometheus text, family by family.
    pub fn render(&self) -> String {
        let mut out = String::new();

        out.push_str("# HELP agenthub_http_requests_total HTTP requests answered, by method and status class.\n");
        out.push_str("# TYPE agenthub_http_requests_total counter\n");
        for (index, method) in METHODS.iter().enumerate() {
            // Class 0 is not a status, so the loop starts at 1.
            for class in 1..STATUS_CLASSES {
                let value = self.http[index][class].load(Ordering::Relaxed);
                out.push_str(&format!(
                    "agenthub_http_requests_total{{method=\"{method}\",status_class=\"{class}xx\"}} {value}\n"
                ));
            }
        }

        out.push_str("# HELP agenthub_events_total Feed events appended, by kind.\n");
        out.push_str("# TYPE agenthub_events_total counter\n");
        for (index, kind) in KINDS.iter().enumerate() {
            let value = self.events[index].load(Ordering::Relaxed);
            out.push_str(&format!(
                "agenthub_events_total{{kind=\"{}\"}} {value}\n",
                escape_label(kind)
            ));
        }

        out.push_str("# HELP agenthub_tool_calls_total MCP tool calls served, by tool name.\n");
        out.push_str("# TYPE agenthub_tool_calls_total counter\n");
        let tools = self.tools.lock().unwrap();
        // Deterministic order, so a scrape is stable and a test can assert on it.
        let mut names: Vec<&String> = tools.keys().collect();
        names.sort();
        for name in names {
            let value = tools[name];
            out.push_str(&format!(
                "agenthub_tool_calls_total{{tool=\"{}\"}} {value}\n",
                escape_label(name)
            ));
        }

        out
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// The one registry this process keeps, created on first use.
static METRICS: OnceLock<Metrics> = OnceLock::new();

/// The process-wide registry.
pub fn global() -> &'static Metrics {
    METRICS.get_or_init(Metrics::new)
}

/// Count one HTTP request in the global registry.
pub fn record_http(method: &str, status_class: u16) {
    global().record_http(method, status_class);
}

/// Count one appended event in the global registry.
pub fn record_event(kind: &str) {
    global().record_event(kind);
}

/// Count one MCP tool call in the global registry.
pub fn record_tool(tool: &str) {
    global().record_tool(tool);
}

/// Render the global registry as Prometheus text.
pub fn render() -> String {
    global().render()
}

/// The slot for an HTTP method, folding anything unknown into `OTHER`.
fn method_slot(method: &str) -> usize {
    METHODS
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(method))
        .unwrap_or(METHODS.len() - 1)
}

/// Escape a Prometheus label value: backslash, double quote, and newline.
fn escape_label(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_count_matches_the_store() {
        assert_eq!(
            KINDS.len(),
            EVENT_KINDS,
            "the metrics kind set drifted from the store's"
        );
    }

    #[test]
    fn counters_move_and_render() {
        let metrics = Metrics::new();
        metrics.record_http("GET", 2);
        metrics.record_http("GET", 2);
        metrics.record_http("post", 5);
        metrics.record_event("signal");
        metrics.record_event("signal");
        metrics.record_tool("whoami");
        metrics.record_tool(UNKNOWN_TOOL);

        let text = metrics.render();
        assert!(
            text.contains("agenthub_http_requests_total{method=\"GET\",status_class=\"2xx\"} 2")
        );
        assert!(
            text.contains("agenthub_http_requests_total{method=\"POST\",status_class=\"5xx\"} 1")
        );
        assert!(text.contains("agenthub_events_total{kind=\"signal\"} 2"));
        assert!(text.contains("agenthub_tool_calls_total{tool=\"whoami\"} 1"));
        assert!(text.contains("agenthub_tool_calls_total{tool=\"<unknown>\"} 1"));
    }

    #[test]
    fn an_unknown_kind_and_status_are_ignored() {
        let metrics = Metrics::new();
        metrics.record_event("not-a-kind");
        metrics.record_http("GET", 9);
        assert!(!metrics.render().contains("not-a-kind"));
        assert!(
            metrics
                .render()
                .contains("agenthub_http_requests_total{method=\"GET\",status_class=\"2xx\"} 0")
        );
    }

    #[test]
    fn a_label_value_is_escaped() {
        assert_eq!(escape_label("a\"b\\c"), "a\\\"b\\\\c");
    }
}
