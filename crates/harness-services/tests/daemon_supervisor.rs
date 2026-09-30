//! Daemon task isolation (restart with backoff, failures logged) and the single-instance guard.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use harness_services::daemon::supervisor::{supervise, Backoff, TaskKind};
use serde_json::Value;

fn fast_backoff() -> Backoff {
    Backoff {
        initial: Duration::from_millis(5),
        max: Duration::from_millis(40),
    }
}

fn records(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn backoff_doubles_from_one_second_to_sixty() {
    let backoff = Backoff::DEFAULT;
    let mut delay = backoff.initial;
    let mut delays = vec![delay.as_secs()];
    for _ in 0..7 {
        delay = backoff.after(delay);
        delays.push(delay.as_secs());
    }
    assert_eq!(delays, vec![1, 2, 4, 8, 16, 32, 60, 60]);
}

#[tokio::test(flavor = "current_thread")]
async fn panicking_and_failing_attempts_are_logged_and_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("services.lifecycle.jsonl");
    let attempts = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&attempts);
    supervise(
        "fixture",
        TaskKind::OneShot,
        log.clone(),
        fast_backoff(),
        move || {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                match attempt {
                    0 => panic!("fixture panic"),
                    1 => Err(anyhow::anyhow!("fixture error")),
                    _ => Ok(()),
                }
            }
        },
    )
    .await;

    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    let failures: Vec<Value> = records(&log)
        .into_iter()
        .filter(|record| record["operation"] == "daemon.task.failed")
        .collect();
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0]["details"]["task"], "fixture");
    assert_eq!(failures[0]["details"]["kind"], "panic");
    assert_eq!(failures[0]["details"]["error"], "fixture panic");
    assert_eq!(failures[1]["details"]["kind"], "error");
    assert_eq!(failures[1]["details"]["error"], "fixture error");
}

#[tokio::test(flavor = "current_thread")]
async fn long_running_task_that_returns_is_restarted_and_logged_once() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("services.lifecycle.jsonl");
    let attempts = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&attempts);
    let supervised = supervise(
        "quiet",
        TaskKind::LongRunning,
        log.clone(),
        fast_backoff(),
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        },
    );
    let stopped = tokio::time::timeout(Duration::from_millis(200), supervised).await;

    assert!(stopped.is_err(), "a long-running supervisor never returns");
    assert!(attempts.load(Ordering::SeqCst) >= 3);
    let exits = records(&log)
        .into_iter()
        .filter(|record| record["operation"] == "daemon.task.exited")
        .count();
    assert_eq!(exits, 1);
}

/// The Windows guard is a session-wide named mutex that a real daemon may hold; the lock-file guard is
/// per harness root, so it is safe to exercise in a temp root.
#[cfg(not(windows))]
#[test]
fn only_one_daemon_instance_per_root() {
    use harness_services::daemon::instance::{acquire, is_daemon_running};
    use harness_services::service_context::ServiceContext;

    let dir = tempfile::tempdir().unwrap();
    let context = ServiceContext {
        harness_root: dir.path().to_path_buf(),
        env: Default::default(),
    };
    assert!(!is_daemon_running(&context));

    let first = acquire(&context).unwrap();
    assert!(first.is_some());
    assert!(acquire(&context).unwrap().is_none());
    assert!(is_daemon_running(&context));

    drop(first);
    assert!(!is_daemon_running(&context));
}
