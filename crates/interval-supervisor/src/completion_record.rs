//! The ONE JSON line printed at completion (interval_supervisor.py:86 `meaningful_tail`,
//! :135 `completion_record`, :215-224 the start-failure record).
//!
//! Key order is Python's dict order: label, status, exit_code, duration_seconds, summary, tail,
//! stdout_log, stderr_log.

use std::collections::VecDeque;
use std::path::Path;

use serde_json::{Map, Value};

use crate::failure_summary::select_failure_summary;

const SUCCEEDED: &str = "succeeded";
const FAILED: &str = "failed";
const DEFAULT_SUCCESS_SUMMARY: &str = "completed";
const DEFAULT_FAILURE_SUMMARY: &str = "command failed";

/// What a finished child left behind.
pub struct ChildOutcome<'a> {
    pub label: &'a str,
    pub exit_code: i64,
    pub duration_seconds: f64,
    pub stdout_lines: &'a [String],
    pub stderr_lines: &'a [String],
}

/// Tail sizes and file locations from the CLI.
pub struct RecordLayout<'a> {
    pub success_tail_lines: usize,
    pub failure_tail_lines: usize,
    pub stdout_path: &'a Path,
    pub stderr_path: &'a Path,
}

/// Why the child could not be started (Python exception type name and `str(error)`).
pub struct StartFailure {
    pub exception_type: String,
    pub message: String,
}

/// Mental model: success → last meaningful lines of the preferred stream; failure → a bounded
/// share of BOTH streams plus the highest-ranked failure line as the summary.
pub fn completion_record(
    outcome: &ChildOutcome<'_>,
    layout: &RecordLayout<'_>,
) -> Map<String, Value> {
    let succeeded = outcome.exit_code == 0;
    let (tail, summary) = if succeeded {
        success_tail_and_summary(outcome, layout.success_tail_lines)
    } else {
        failure_tail_and_summary(outcome, layout.failure_tail_lines)
    };

    record_map(
        outcome.label,
        if succeeded { SUCCEEDED } else { FAILED },
        Value::from(outcome.exit_code),
        outcome.duration_seconds,
        summary,
        tail,
        layout,
    )
}

/// The record printed when `Popen` raised (exit_code null).
pub fn start_failure_record(
    label: &str,
    duration_seconds: f64,
    failure: &StartFailure,
    max_line_chars: usize,
    layout: &RecordLayout<'_>,
) -> Map<String, Value> {
    let detail: String = failure.message.chars().take(max_line_chars).collect();
    record_map(
        label,
        FAILED,
        Value::Null,
        duration_seconds,
        format!("command could not start: {}", failure.exception_type),
        vec![detail],
        layout,
    )
}

fn success_tail_and_summary(outcome: &ChildOutcome<'_>, limit: usize) -> (Vec<String>, String) {
    let preferred = if outcome.stdout_lines.is_empty() {
        outcome.stderr_lines
    } else {
        outcome.stdout_lines
    };
    let tail = meaningful_tail(preferred, limit);
    let summary = tail
        .last()
        .cloned()
        .unwrap_or_else(|| DEFAULT_SUCCESS_SUMMARY.to_owned());
    (tail, summary)
}

/// Toolchains disagree about which stream owns the actionable failure, so both keep a share:
/// stdout `max(1, n // 2)`, stderr `max(1, n - stdout_share)`.
fn failure_tail_and_summary(outcome: &ChildOutcome<'_>, limit: usize) -> (Vec<String>, String) {
    let stdout_limit = (limit / 2).max(1);
    let stderr_limit = limit.saturating_sub(stdout_limit).max(1);
    let mut tail = meaningful_tail(outcome.stdout_lines, stdout_limit);
    tail.extend(meaningful_tail(outcome.stderr_lines, stderr_limit));

    let all_lines: Vec<String> = outcome
        .stdout_lines
        .iter()
        .chain(outcome.stderr_lines)
        .cloned()
        .collect();
    let mut summary = select_failure_summary(&all_lines);
    if summary.is_empty() {
        summary = tail
            .last()
            .cloned()
            .unwrap_or_else(|| DEFAULT_FAILURE_SUMMARY.to_owned());
    }
    (tail, summary)
}

/// Non-empty lines, consecutive duplicates collapsed, newest `limit` kept.
pub fn meaningful_tail(lines: &[String], limit: usize) -> Vec<String> {
    let mut result: VecDeque<String> = VecDeque::new();
    for line in lines {
        if line.is_empty() || result.back() == Some(line) {
            continue;
        }
        if result.len() == limit {
            result.pop_front();
        }
        result.push_back(line.clone());
    }
    result.into_iter().collect()
}

/// Python `round(value, 3)`: correctly rounded to the nearest decimal with 3 fraction digits.
#[inline]
pub fn round_to_milliseconds(seconds: f64) -> f64 {
    format!("{seconds:.3}").parse().unwrap_or(seconds)
}

/// Python `str(path)`.
#[inline]
pub fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn record_map(
    label: &str,
    status: &str,
    exit_code: Value,
    duration_seconds: f64,
    summary: String,
    tail: Vec<String>,
    layout: &RecordLayout<'_>,
) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert("label".into(), Value::from(label));
    record.insert("status".into(), Value::from(status));
    record.insert("exit_code".into(), exit_code);
    record.insert(
        "duration_seconds".into(),
        Value::from(round_to_milliseconds(duration_seconds)),
    );
    record.insert("summary".into(), Value::from(summary));
    record.insert("tail".into(), Value::from(tail));
    record.insert(
        "stdout_log".into(),
        Value::from(path_text(layout.stdout_path)),
    );
    record.insert(
        "stderr_log".into(),
        Value::from(path_text(layout.stderr_path)),
    );
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn layout<'a>(
        stdout: &'a Path,
        stderr: &'a Path,
        success: usize,
        failure: usize,
    ) -> RecordLayout<'a> {
        RecordLayout {
            success_tail_lines: success,
            failure_tail_lines: failure,
            stdout_path: stdout,
            stderr_path: stderr,
        }
    }

    #[test]
    fn meaningful_tail_deduplicates_consecutive_progress() {
        assert_eq!(
            meaningful_tail(&lines(&["running", "running", "done"]), 2),
            ["running", "done"]
        );
    }

    #[test]
    fn success_record_returns_only_configured_tail() {
        let (out, err) = (PathBuf::from("out.log"), PathBuf::from("err.log"));
        let stdout = lines(&["progress", "All tests passed!"]);
        let record = completion_record(
            &ChildOutcome {
                label: "test",
                exit_code: 0,
                duration_seconds: 1.2345,
                stdout_lines: &stdout,
                stderr_lines: &[],
            },
            &layout(&out, &err, 1, 20),
        );

        assert_eq!(record["status"], "succeeded");
        assert_eq!(record["exit_code"], 0);
        assert_eq!(record["tail"], serde_json::json!(["All tests passed!"]));
        assert_eq!(record["summary"], "All tests passed!");
        assert_eq!(record["duration_seconds"], 1.234);
    }

    #[test]
    fn failure_record_keeps_bounded_context_from_both_streams() {
        let (out, err) = (PathBuf::from("out.log"), PathBuf::from("err.log"));
        let stdout = lines(&["progress", "Error: server timed out"]);
        let stderr = lines(&["warning", "warning details"]);
        let record = completion_record(
            &ChildOutcome {
                label: "test",
                exit_code: 7,
                duration_seconds: 2.0,
                stdout_lines: &stdout,
                stderr_lines: &stderr,
            },
            &layout(&out, &err, 2, 2),
        );

        assert_eq!(record["status"], "failed");
        assert_eq!(record["exit_code"], 7);
        assert_eq!(
            record["tail"],
            serde_json::json!(["Error: server timed out", "warning details"])
        );
        assert_eq!(record["summary"], "Error: server timed out");
        let keys: Vec<&str> = record.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "label",
                "status",
                "exit_code",
                "duration_seconds",
                "summary",
                "tail",
                "stdout_log",
                "stderr_log"
            ]
        );
    }

    #[test]
    fn empty_runs_use_default_summaries() {
        let (out, err) = (PathBuf::from("o"), PathBuf::from("e"));
        let success = completion_record(
            &ChildOutcome {
                label: "t",
                exit_code: 0,
                duration_seconds: 0.0,
                stdout_lines: &[],
                stderr_lines: &[],
            },
            &layout(&out, &err, 2, 20),
        );
        let failure = completion_record(
            &ChildOutcome {
                label: "t",
                exit_code: 1,
                duration_seconds: 0.0,
                stdout_lines: &[],
                stderr_lines: &[],
            },
            &layout(&out, &err, 2, 20),
        );
        assert_eq!(success["summary"], "completed");
        assert_eq!(failure["summary"], "command failed");
    }

    #[test]
    fn rounds_like_python() {
        assert_eq!(round_to_milliseconds(2.0), 2.0);
        assert_eq!(round_to_milliseconds(0.0015), 0.002);
        assert_eq!(round_to_milliseconds(1.0005), 1.0);
    }
}
