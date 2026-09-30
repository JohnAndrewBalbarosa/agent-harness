//! Every router lifecycle record, with the exact operation names and detail keys/order of
//! `core/router/harness_hook.py` (component `router`).

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::consumer::ConsumerResult;
use crate::events::Event;
use crate::shared_state::lifecycle_log::{details, emit};

const COMPONENT: &str = "router";

/// harness_hook.py:100.
pub fn agent_unknown(log: &Path, agent: &str) {
    emit(
        log,
        COMPONENT,
        "hook.agent.unknown",
        "failed",
        details([("agent", json!(agent))]),
    );
}

/// harness_hook.py:104.
pub fn payload_invalid(log: &Path, agent: &str, native_event: Option<&str>) {
    let pairs = [
        ("agent", json!(agent)),
        ("nativeEvent", json!(native_event)),
    ];
    emit(
        log,
        COMPONENT,
        "hook.payload.invalid",
        "failed",
        details(pairs),
    );
}

/// harness_hook.py:90.
pub fn version_untested(log: &Path, agent: &str, version: &str) {
    let pairs = [("agent", json!(agent)), ("version", json!(version))];
    emit(
        log,
        COMPONENT,
        "agent.version.untested",
        "observed",
        details(pairs),
    );
}

/// harness_hook.py:56 (the Rust analogue: the consumer thread could not be started).
pub fn consumer_launch_failed(log: &Path, consumer: &str, error: &str) {
    let pairs = [("consumer", json!(consumer)), ("error", json!(error))];
    emit(
        log,
        COMPONENT,
        "consumer.launch_failed",
        "failed",
        details(pairs),
    );
}

/// harness_hook.py:63.
pub fn consumer_timeout(log: &Path, consumer: &str, timeout: Duration) {
    let pairs = [
        ("consumer", json!(consumer)),
        ("timeoutS", seconds_value(timeout)),
    ];
    emit(log, COMPONENT, "consumer.timeout", "failed", details(pairs));
}

/// harness_hook.py:65.
/// `error` is added only for failed consumers (the Python router discarded consumer stderr, so failures
/// were silent); it is bounded and never contains payload data, only the error chain text.
pub fn consumer_exited(
    log: &Path,
    consumer: &str,
    exit_code: i32,
    duration: Duration,
    error: Option<&str>,
) {
    let mut record = details([
        ("consumer", json!(consumer)),
        ("exitCode", json!(exit_code)),
        ("durationMs", milliseconds_value(duration)),
    ]);
    if let Some(error) = error {
        record.insert("error".into(), json!(error));
    }
    emit(
        log,
        COMPONENT,
        "consumer.exited",
        outcome_for(exit_code),
        record,
    );
}

/// harness_hook.py:115.
pub fn dispatched(
    log: &Path,
    agent: &str,
    event: &Event,
    native: Option<&str>,
    results: &[ConsumerResult],
    started: Instant,
) {
    let names: Vec<Value> = results.iter().map(|result| json!(result.name)).collect();
    let failures: Map<String, Value> = results
        .iter()
        .filter(|result| result.exit_code != 0)
        .map(|result| (result.name.to_owned(), json!(result.exit_code)))
        .collect();
    let pairs = [
        ("agent", json!(agent)),
        ("instance", json!(event.instance)),
        ("event", json!(event.event)),
        ("nativeEvent", json!(native)),
        ("consumers", Value::Array(names)),
        ("failures", Value::Object(failures)),
        ("durationMs", milliseconds_value(started.elapsed())),
    ];
    emit(log, COMPONENT, "hook.dispatch", "succeeded", details(pairs));
}

/// harness_hook.py:121 (`error` is the Python exception class name; the Rust path only fails by panic).
pub fn dispatch_failed(log: &Path, agent: &str, native: Option<&str>, error: &str) {
    let pairs = [
        ("agent", json!(agent)),
        ("nativeEvent", json!(native)),
        ("error", json!(error)),
    ];
    emit(log, COMPONENT, "hook.dispatch", "failed", details(pairs));
}

#[inline]
fn outcome_for(exit_code: i32) -> &'static str {
    if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    }
}

/// `round((time.perf_counter() - started) * 1000, 1)` (always a float).
#[inline]
fn milliseconds_value(duration: Duration) -> Value {
    let milliseconds = duration.as_secs_f64() * 1000.0;
    json!((milliseconds * 10.0).round() / 10.0)
}

/// `timeoutS=consumer.timeout_s`: an int in Python; fractional Rust timeouts stay floats.
#[inline]
fn seconds_value(timeout: Duration) -> Value {
    if timeout.subsec_nanos() == 0 {
        json!(timeout.as_secs())
    } else {
        json!(timeout.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_to_one_decimal_float() {
        assert_eq!(
            milliseconds_value(Duration::from_micros(12_345)),
            json!(12.3)
        );
        assert!(milliseconds_value(Duration::ZERO).is_f64());
    }

    #[test]
    fn whole_second_timeouts_are_integers() {
        assert_eq!(seconds_value(Duration::from_secs(25)), json!(25));
    }
}
