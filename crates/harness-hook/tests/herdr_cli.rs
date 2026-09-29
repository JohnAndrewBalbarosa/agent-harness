//! `ProcessHerdrRunner` against a fake `herdr` script (`hook.py` `run_herdr`): merged output, exit codes,
//! timeout → 124, launch failure → 1, lifecycle records.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_hook::consumers::herdr_bridge::bridge_log::BridgeLog;
use harness_hook::consumers::herdr_bridge::herdr_cli::{
    HerdrRunner, ProcessHerdrRunner, TIMEOUT_EXIT_CODE,
};

fn fake_herdr(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("herdr");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[test]
fn merges_stderr_keeps_the_tail_and_logs_the_exit() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_herdr(dir.path(), "echo \"out $1\"; echo err 1>&2; exit 3");
    let log = BridgeLog::new(dir.path().join("bridge.jsonl"));

    let result =
        ProcessHerdrRunner::new(binary, &log).run(&args(&["pane", "x"]), Duration::from_secs(5), 7);

    assert_eq!(result.exit_code, 3);
    // Merged "out pane\nerr\n", last 7 code points.
    assert_eq!(result.output, "ne\nerr\n");
    let text = std::fs::read_to_string(log.path()).unwrap();
    assert!(text.contains("\"event\":\"helper.process.started\""));
    assert!(text.contains(
        "\"event\":\"helper.process.exited\",\"outcome\":\"failed\",\"operation\":\"pane\""
    ));
    assert!(text.contains("\"exitCode\":3"));
}

#[test]
fn full_output_when_under_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_herdr(dir.path(), "printf 'a\\r\\nb'; printf 'c' 1>&2");
    let log = BridgeLog::new(dir.path().join("bridge.jsonl"));

    let result =
        ProcessHerdrRunner::new(binary, &log).run(&args(&["agent"]), Duration::from_secs(5), 1000);

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, "a\nbc");
}

#[test]
fn timeout_kills_the_process_and_reports_124() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_herdr(dir.path(), "exec sleep 5");
    let log = BridgeLog::new(dir.path().join("bridge.jsonl"));

    let result = ProcessHerdrRunner::new(binary, &log).run(
        &args(&["pane"]),
        Duration::from_millis(200),
        1000,
    );

    assert_eq!(result.exit_code, TIMEOUT_EXIT_CODE);
    let text = std::fs::read_to_string(log.path()).unwrap();
    assert!(text.contains("\"reason\":\"timeout\""));
}

#[test]
fn missing_binary_is_a_launch_failure() {
    let dir = tempfile::tempdir().unwrap();
    let log = BridgeLog::new(dir.path().join("bridge.jsonl"));

    let result = ProcessHerdrRunner::new(dir.path().join("nope"), &log).run(
        &args(&["pane"]),
        Duration::from_secs(1),
        1000,
    );

    assert_eq!(result.exit_code, 1);
    let text = std::fs::read_to_string(log.path()).unwrap();
    assert!(text.contains("\"reason\":\"launch_failed\",\"errorType\":\"FileNotFoundError\""));
}
