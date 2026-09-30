//! End-to-end behaviour of the `interval-supervisor` binary, ported from
//! `~/.codex-shared/tools/observability-hub/tests/test_interval_supervisor.py` plus the contract
//! points that test left implicit (exit codes, start failure, final state, usage errors, events).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_interval-supervisor");

/// A fake child: `sh -c` on POSIX, `cmd /C` on Windows.
fn shell_command(posix_script: &str, windows_script: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), windows_script.into()]
    } else {
        vec!["sh".into(), "-c".into(), posix_script.into()]
    }
}

fn run_supervisor(root: &Path, label: &str, options: &[&str], command: &[String]) -> Output {
    Command::new(SUPERVISOR)
        .args(["--label", label, "--interval", "5", "--cwd"])
        .arg(root)
        .args(["--log-dir", "logs"])
        .args(options)
        .arg("--")
        .args(command)
        .env_remove("AH_OBS_SPOOL")
        .output()
        .expect("supervisor starts")
}

fn single_record(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one stdout line expected, got {stdout:?}"
    );
    serde_json::from_str(lines[0]).expect("completion record is JSON")
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("log file exists")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn runner_emits_one_json_summary_and_bounds_persisted_output() {
    let root = tempfile::tempdir().unwrap();
    let command = shell_command(
        "echo one; echo two; echo three; echo SUCCESS",
        "echo one&echo two&echo three&echo SUCCESS",
    );

    let output = run_supervisor(
        root.path(),
        "bounded-success",
        &["--max-log-lines", "3", "--success-tail-lines", "1"],
        &command,
    );
    let record = single_record(&output);
    let persisted = read_lines(&root.path().join("logs").join("bounded-success.stdout.log"));

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(record["status"], "succeeded");
    assert_eq!(record["exit_code"], 0);
    assert_eq!(record["tail"], serde_json::json!(["SUCCESS"]));
    assert_eq!(record["summary"], "SUCCESS");
    assert_eq!(persisted, ["two", "three", "SUCCESS"]);
}

#[test]
fn failure_forwards_exit_code_and_keeps_both_streams() {
    let root = tempfile::tempdir().unwrap();
    let command = shell_command(
        "echo progress; echo 'Error: server timed out'; echo warning >&2; echo 'warning details' >&2; exit 7",
        "echo progress&echo Error: server timed out&echo warning 1>&2&echo warning details 1>&2&exit /b 7",
    );

    let output = run_supervisor(
        root.path(),
        "failing",
        &["--failure-tail-lines", "2"],
        &command,
    );
    let record = single_record(&output);

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(record["status"], "failed");
    assert_eq!(record["exit_code"], 7);
    assert_eq!(
        record["tail"],
        serde_json::json!(["Error: server timed out", "warning details"])
    );
    assert_eq!(record["summary"], "Error: server timed out");
    let keys: Vec<&str> = record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
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
fn start_failure_reports_null_exit_code_and_exits_one() {
    let root = tempfile::tempdir().unwrap();
    let command = vec!["definitely-not-a-real-program-4f2c".to_owned()];

    let output = run_supervisor(root.path(), "no start", &[], &command);
    let record = single_record(&output);
    let logs = root.path().join("logs");
    let state: Value =
        serde_json::from_str(&fs::read_to_string(logs.join("no_start.state.json")).unwrap())
            .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(record["label"], "no_start");
    assert_eq!(record["status"], "failed");
    assert_eq!(record["exit_code"], Value::Null);
    assert_eq!(
        record["summary"],
        "command could not start: FileNotFoundError"
    );
    assert_eq!(read_lines(&logs.join("no_start.stderr.log")).len(), 1);
    assert_eq!(
        fs::read_to_string(logs.join("no_start.stdout.log")).unwrap(),
        ""
    );
    assert_eq!(state, record);
}

#[test]
fn final_state_merges_running_state_with_the_record() {
    let root = tempfile::tempdir().unwrap();
    let command = shell_command("echo done", "echo done");

    let output = run_supervisor(root.path(), "state", &[], &command);
    let record = single_record(&output);
    let state: Value = serde_json::from_str(
        &fs::read_to_string(root.path().join("logs").join("state.state.json")).unwrap(),
    )
    .unwrap();
    let keys: Vec<&str> = state
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(state["status"], "succeeded");
    assert!(state["pid"].as_u64().is_some());
    assert_eq!(state["intervalSeconds"], 5);
    assert_eq!(state["summary"], record["summary"]);
    assert_eq!(
        keys,
        [
            "label",
            "pid",
            "status",
            "intervalSeconds",
            "stdout",
            "stderr",
            "startedAt",
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
fn usage_errors_exit_two_without_stdout() {
    let output = Command::new(SUPERVISOR)
        .args(["--label", "x", "--interval", "4", "--", "echo"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(stderr.starts_with("usage: "));
    assert!(stderr.contains(": error: interval must be at least 5 seconds"));

    let missing_terminator = Command::new(SUPERVISOR)
        .args(["--label", "x", "echo"])
        .output()
        .unwrap();
    assert_eq!(missing_terminator.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing_terminator.stderr)
        .contains(": error: command must follow --"));
}

#[cfg(unix)]
#[test]
fn progress_frames_and_ansi_are_normalized() {
    let root = tempfile::tempdir().unwrap();
    let command = shell_command(
        r"printf 'loading 90%%\r\033[32mAll tests passed!\033[0m\n'",
        "",
    );

    let output = run_supervisor(
        root.path(),
        "ansi",
        &["--success-tail-lines", "1"],
        &command,
    );
    let record = single_record(&output);

    assert_eq!(record["tail"], serde_json::json!(["All tests passed!"]));
    assert!(!String::from_utf8_lossy(&output.stdout).contains('\u{1b}'));
}

#[test]
fn project_id_events_are_spooled_without_affecting_the_run() {
    let root = tempfile::tempdir().unwrap();
    let spool = root.path().join("spool.sqlite3");
    let command = shell_command("echo ok", "echo ok");

    let output = Command::new(SUPERVISOR)
        .args(["--label", "obs", "--interval", "5", "--cwd"])
        .arg(root.path())
        .args(["--project-id", "11111111-1111-1111-1111-111111111111", "--"])
        .args(&command)
        .env("AH_OBS_SPOOL", &spool)
        // Nothing listens on the discard port: delivery fails, operations stay spooled.
        .env("OBS_ENDPOINT", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let record = single_record(&output);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(record["status"], "succeeded");

    let connection = obs_cli::spool::open::open_spool_at(&spool).unwrap();
    let mut statement = connection
        .prepare("SELECT payload FROM queue UNION ALL SELECT payload FROM dead_letter")
        .unwrap();
    let payloads: Vec<Value> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str(&payload.unwrap()).unwrap())
        .collect();
    let event_codes: Vec<&str> = payloads
        .iter()
        .filter(|operation| operation["kind"] == "event.emit")
        .filter_map(|operation| operation["data"]["code"].as_str())
        .collect();
    let kinds: Vec<&str> = payloads
        .iter()
        .filter_map(|operation| operation["kind"].as_str())
        .collect();

    assert!(event_codes.contains(&"process.started"));
    assert!(event_codes.contains(&"process.completed"));
    assert!(kinds.contains(&"execution.start"));
    assert!(kinds.contains(&"execution.finish"));
}
