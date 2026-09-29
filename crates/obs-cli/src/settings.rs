//! Environment knobs honoured by obs.py (obs.py:29-32), clamped the same way.
//!
//! Deviation: an unparseable number falls back to the default instead of crashing at import.

use std::env;
use std::time::Duration;

use crate::python_compat::{parse_python_float, parse_python_int};

const ENDPOINT_ENV: &str = "OBS_ENDPOINT";
const MAX_QUEUED_ENV: &str = "OBS_MAX_QUEUED_OPERATIONS";
const MAX_DEAD_LETTER_ENV: &str = "OBS_MAX_DEAD_LETTER_OPERATIONS";
const HTTP_TIMEOUT_ENV: &str = "OBS_HTTP_TIMEOUT_SECONDS";

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4319";
const DEFAULT_MAX_QUEUED: i64 = 10_000;
const DEFAULT_MAX_DEAD_LETTER: i64 = 1_000;
const MIN_OPERATION_CAP: i64 = 100;
const DEFAULT_HTTP_TIMEOUT_SECONDS: f64 = 15.0;
const MIN_HTTP_TIMEOUT_SECONDS: f64 = 2.5;
const MAX_HTTP_TIMEOUT_SECONDS: f64 = 60.0;

/// obs.py:860 `fetch_json` uses a fixed 5 s timeout for reads.
pub const HUB_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Settings {
    pub endpoint: String,
    pub max_queued_operations: i64,
    pub max_dead_letter_operations: i64,
    pub http_timeout: Duration,
}

impl Settings {
    pub fn from_env() -> Self {
        Self {
            endpoint: env::var(ENDPOINT_ENV).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned()),
            max_queued_operations: operation_cap(MAX_QUEUED_ENV, DEFAULT_MAX_QUEUED),
            max_dead_letter_operations: operation_cap(MAX_DEAD_LETTER_ENV, DEFAULT_MAX_DEAD_LETTER),
            http_timeout: http_timeout(),
        }
    }
}

/// `max(100, int(env))`.
fn operation_cap(name: &str, default: i64) -> i64 {
    let requested = env::var(name)
        .ok()
        .and_then(|text| parse_python_int(&text))
        .unwrap_or(default);
    requested.max(MIN_OPERATION_CAP)
}

/// `max(2.5, min(60.0, float(env)))`, including Python's NaN/inf outcomes.
fn http_timeout() -> Duration {
    let requested = env::var(HTTP_TIMEOUT_ENV)
        .ok()
        .and_then(|text| parse_python_float(&text))
        .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECONDS);
    Duration::from_secs_f64(clamp_like_python(requested))
}

#[inline]
fn clamp_like_python(requested: f64) -> f64 {
    let capped = if requested < MAX_HTTP_TIMEOUT_SECONDS {
        requested
    } else {
        MAX_HTTP_TIMEOUT_SECONDS
    };
    if capped > MIN_HTTP_TIMEOUT_SECONDS {
        capped
    } else {
        MIN_HTTP_TIMEOUT_SECONDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_timeout_like_python_min_max() {
        assert_eq!(clamp_like_python(1.0), 2.5);
        assert_eq!(clamp_like_python(90.0), 60.0);
        assert_eq!(clamp_like_python(f64::NAN), 60.0);
        assert_eq!(clamp_like_python(f64::NEG_INFINITY), 2.5);
        assert_eq!(clamp_like_python(15.0), 15.0);
    }
}
