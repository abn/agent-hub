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
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::error::ErrorCode;
use crate::store::events::KINDS;

/// The fixed HTTP methods the counter distinguishes. Everything else, including
/// a method this build does not know, lands in the `other` slot.
const METHODS: [&str; 8] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "OTHER",
];

/// Status classes 1xx through 5xx. The index is the class, so slot 0 is unused
/// and a class outside 1 to 5 is not a series that can appear.
const STATUS_CLASSES: usize = 6;

/// Every hub error code, in the order the counter indexes them. A code outside
/// this closed set cannot be minted, so the family cannot grow with caller
/// input.
const ERROR_CODES: [ErrorCode; 9] = [
    ErrorCode::InvalidArgument,
    ErrorCode::Unauthenticated,
    ErrorCode::Forbidden,
    ErrorCode::NotFound,
    ErrorCode::Conflict,
    ErrorCode::PayloadTooLarge,
    ErrorCode::RateLimited,
    ErrorCode::Unavailable,
    ErrorCode::Internal,
];

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
    /// One slot per code in [`ERROR_CODES`], same order. Every failed response
    /// the HTTP and MCP layers answer is counted here, so a failing family
    /// (storage `unavailable` above all) is visible on a scrape.
    errors: [AtomicU64; ERROR_CODES.len()],
    /// Tool name to count. Bounded by [`TOOL_LABELS_MAX`].
    tools: Mutex<HashMap<String, u64>>,
    /// 1 when the last background integrity sample passed, 0 when it failed.
    /// Negative means no sample has run yet, so a scrape before the first
    /// interval does not report a failure the hub never had.
    integrity_ok: AtomicI64,
    /// Integrity samples that failed, by timeout or error or a reported issue.
    integrity_failures: AtomicU64,
    /// Notify sends to the operator's target: slot 0 delivered, slot 1 failed.
    notify: [AtomicU64; 2],
}

impl Metrics {
    /// A registry with every counter at zero.
    pub fn new() -> Self {
        Self {
            http: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            events: std::array::from_fn(|_| AtomicU64::new(0)),
            errors: std::array::from_fn(|_| AtomicU64::new(0)),
            tools: Mutex::new(HashMap::new()),
            integrity_ok: AtomicI64::new(-1),
            integrity_failures: AtomicU64::new(0),
            notify: std::array::from_fn(|_| AtomicU64::new(0)),
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

    /// Count one failed response by its hub error code. The code is a closed
    /// set, so the family stays bounded.
    pub fn record_error(&self, code: ErrorCode) {
        if let Some(slot) = ERROR_CODES.iter().position(|candidate| *candidate == code) {
            self.errors[slot].fetch_add(1, Ordering::Relaxed);
        }
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

    /// Record one completed background integrity sample. A pass sets the gauge
    /// to 1; a failure sets it to 0 and moves the failure counter.
    pub fn record_integrity(&self, ok: bool) {
        self.integrity_ok.store(i64::from(ok), Ordering::Relaxed);
        if !ok {
            self.integrity_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Count one send to the operator's notify target.
    pub fn record_notify(&self, delivered: bool) {
        self.notify[usize::from(!delivered)].fetch_add(1, Ordering::Relaxed);
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

        out.push_str("# HELP agenthub_errors_total Failed responses, by hub error code.\n");
        out.push_str("# TYPE agenthub_errors_total counter\n");
        for (index, code) in ERROR_CODES.iter().enumerate() {
            let value = self.errors[index].load(Ordering::Relaxed);
            out.push_str(&format!(
                "agenthub_errors_total{{code=\"{}\"}} {value}\n",
                code.as_str()
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

        out.push_str(
            "# HELP agenthub_notify_sends_total Sends to the operator's notify target, by result.\n",
        );
        out.push_str("# TYPE agenthub_notify_sends_total counter\n");
        for (index, result) in ["delivered", "failed"].iter().enumerate() {
            out.push_str(&format!(
                "agenthub_notify_sends_total{{result=\"{result}\"}} {}\n",
                self.notify[index].load(Ordering::Relaxed)
            ));
        }

        // The integrity gauge is 1 or 0 once a sample has run. Before the first
        // sample the family is omitted rather than read as a failure, so a hub
        // that just started is not reported as broken.
        let integrity = self.integrity_ok.load(Ordering::Relaxed);
        if integrity >= 0 {
            out.push_str(
                "# HELP agenthub_integrity_ok Result of the last background store integrity sample: 1 pass, 0 fail.\n",
            );
            out.push_str("# TYPE agenthub_integrity_ok gauge\n");
            out.push_str(&format!("agenthub_integrity_ok {integrity}\n"));

            out.push_str(
                "# HELP agenthub_integrity_failures_total Background integrity samples that failed.\n",
            );
            out.push_str("# TYPE agenthub_integrity_failures_total counter\n");
            out.push_str(&format!(
                "agenthub_integrity_failures_total {}\n",
                self.integrity_failures.load(Ordering::Relaxed)
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

/// Count one failed response in the global registry.
pub fn record_error(code: ErrorCode) {
    global().record_error(code);
}

/// Count one MCP tool call in the global registry.
pub fn record_tool(tool: &str) {
    global().record_tool(tool);
}

/// Record one completed background integrity sample in the global registry.
pub fn record_integrity(ok: bool) {
    global().record_integrity(ok);
}

/// Count one notify send in the global registry.
pub fn record_notify(delivered: bool) {
    global().record_notify(delivered);
}

/// Render the global registry as Prometheus text.
pub fn render() -> String {
    global().render()
}

/// Render the storage gauges for a data directory.
///
/// These are point-in-time readings of the volume, not counters, so they are
/// built fresh per scrape rather than kept in the registry: the free space
/// changes with every write elsewhere on the volume, and the assessment needs a
/// syscall. A value that cannot be measured is omitted, never reported as zero.
pub fn render_gauges(data_dir: &std::path::Path, hub_db: &std::path::Path) -> String {
    let mut out = String::new();

    if let Some(free) = crate::store::free_space_bytes(data_dir) {
        out.push_str("# HELP agenthub_data_volume_free_bytes Free bytes on the data volume.\n");
        out.push_str("# TYPE agenthub_data_volume_free_bytes gauge\n");
        out.push_str(&format!("agenthub_data_volume_free_bytes {free}\n"));
    }

    let store_bytes = file_len(hub_db);
    out.push_str("# HELP agenthub_hub_db_bytes Size of the hub store file in bytes.\n");
    out.push_str("# TYPE agenthub_hub_db_bytes gauge\n");
    out.push_str(&format!("agenthub_hub_db_bytes {store_bytes}\n"));

    // The write-ahead log is folded into the database at a clean shutdown and
    // grows with writes in between; a large value next to a large store is the
    // approach to a full volume an operator wants to trend.
    let wal_bytes = file_len(&with_suffix(hub_db, "-wal"));
    out.push_str(
        "# HELP agenthub_hub_wal_bytes Size of the hub store's write-ahead log in bytes.\n",
    );
    out.push_str("# TYPE agenthub_hub_wal_bytes gauge\n");
    out.push_str(&format!("agenthub_hub_wal_bytes {wal_bytes}\n"));

    out
}

/// A file's length, or zero when it is absent.
fn file_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// A path with a sidecar suffix appended, such as `hub.db-wal`.
fn with_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
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
        metrics.record_error(ErrorCode::Unavailable);
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
        assert!(text.contains("agenthub_errors_total{code=\"unavailable\"} 1"));
        assert!(text.contains("agenthub_tool_calls_total{tool=\"whoami\"} 1"));
        assert!(text.contains("agenthub_tool_calls_total{tool=\"<unknown>\"} 1"));
    }

    #[test]
    fn every_error_code_has_a_series() {
        let metrics = Metrics::new();
        let text = metrics.render();
        for code in ERROR_CODES {
            assert!(
                text.contains(&format!(
                    "agenthub_errors_total{{code=\"{}\"}}",
                    code.as_str()
                )),
                "{} is rendered",
                code.as_str()
            );
        }
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

    #[test]
    fn notify_sends_are_counted_by_result() {
        let metrics = Metrics::new();
        metrics.record_notify(true);
        metrics.record_notify(false);
        metrics.record_notify(false);
        let text = metrics.render();
        assert!(
            text.contains("agenthub_notify_sends_total{result=\"delivered\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("agenthub_notify_sends_total{result=\"failed\"} 2"),
            "{text}"
        );
    }

    #[test]
    fn integrity_is_omitted_before_the_first_sample() {
        let metrics = Metrics::new();
        assert!(
            !metrics.render().contains("agenthub_integrity_ok"),
            "no sample has run, so no gauge is claimed"
        );
    }

    #[test]
    fn integrity_samples_move_the_gauge_and_failure_counter() {
        let metrics = Metrics::new();
        metrics.record_integrity(true);
        let text = metrics.render();
        assert!(
            text.contains("agenthub_integrity_ok 1"),
            "a pass reads 1: {text}"
        );
        assert!(
            text.contains("agenthub_integrity_failures_total 0"),
            "a pass does not move the failure counter: {text}"
        );

        metrics.record_integrity(false);
        let text = metrics.render();
        assert!(
            text.contains("agenthub_integrity_ok 0"),
            "a fail reads 0: {text}"
        );
        assert!(
            text.contains("agenthub_integrity_failures_total 1"),
            "a fail moves the counter: {text}"
        );
    }
}
