//! Every record the monitor writes, with the exact Python `details` keys and key order.

use serde_json::{json, Map, Value};

use super::process_ancestry::Ancestry;
use super::window_filter::truncate_window_class;

pub const SEVERITY_INFO: &str = "info";
pub const SEVERITY_ERROR: &str = "error";

/// One lifecycle record before the timestamp and component are attached.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorRecord {
    pub severity: &'static str,
    pub operation: &'static str,
    pub outcome: &'static str,
    pub details: Map<String, Value>,
}

/// Facts about one reportable console window (no title, no command line).
#[derive(Debug, Clone)]
pub struct VisibleWindow<'a> {
    pub event_id: u32,
    pub pid: u32,
    pub parent_pid: u32,
    pub process: &'a str,
    pub window_class: &'a str,
}

#[inline]
fn record(
    severity: &'static str,
    operation: &'static str,
    outcome: &'static str,
    details: Value,
) -> MonitorRecord {
    let details = match details {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    MonitorRecord {
        severity,
        operation,
        outcome,
        details,
    }
}

/// `monitor.start failed` — the named mutex could not be created (Python line 173).
pub fn start_failed(error_code: u32) -> MonitorRecord {
    record(
        SEVERITY_ERROR,
        "monitor.start",
        "failed",
        json!({"reason": "mutex_unavailable", "errorCode": error_code}),
    )
}

/// `monitor.hook failed` — `SetWinEventHook` returned null (Python line 222).
pub fn hook_failed(event_id: u32, error_code: u32) -> MonitorRecord {
    record(
        SEVERITY_ERROR,
        "monitor.hook",
        "failed",
        json!({"eventId": event_id, "errorCode": error_code}),
    )
}

/// `monitor.start succeeded` (Python line 225).
pub fn started(process_id: u32) -> MonitorRecord {
    record(
        SEVERITY_INFO,
        "monitor.start",
        "succeeded",
        json!({"processId": process_id, "windowMode": "hidden"}),
    )
}

/// `monitor.stop completed` (Python line 234).
pub fn stopped(process_id: u32) -> MonitorRecord {
    record(
        SEVERITY_INFO,
        "monitor.stop",
        "completed",
        json!({"processId": process_id}),
    )
}

/// `console.window.inspect failed` — the callback body failed or panicked (Python line 213).
pub fn inspect_failed(error_type: &str) -> MonitorRecord {
    record(
        SEVERITY_ERROR,
        "console.window.inspect",
        "failed",
        json!({"errorType": error_type}),
    )
}

/// `console.window.visible observed` (Python lines 209-211), keys in Python order.
pub fn window_visible(window: &VisibleWindow<'_>, ancestry: &Ancestry) -> MonitorRecord {
    let chain: Vec<Value> = ancestry
        .chain
        .iter()
        .map(|link| json!({"pid": link.pid, "process": link.process, "parentPid": link.parent_pid}))
        .collect();
    record(
        SEVERITY_INFO,
        "console.window.visible",
        "observed",
        json!({
            "eventId": window.event_id,
            "pid": window.pid,
            "parentPid": window.parent_pid,
            "process": window.process,
            "windowClass": truncate_window_class(window.window_class),
            "profile": ancestry.profile,
            "codexAncestorPid": ancestry.codex_pid,
            "ancestry": chain,
        }),
    )
}
