//! flush_spool.py behaviour of the daemon's drain round, plus request coalescing.

use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use harness_services::ensure::file_guard::try_exclusive;
use harness_services::spool_drain::drain_round::{
    drain_spool, DrainLimits, DrainStatus, SpoolPaths,
};
use harness_services::spool_drain::{channel, DrainTrigger};
use obs_cli::hub_client::DeliveryError;
use obs_cli::settings::Settings;
use obs_cli::spool::flush::OperationSink;
use obs_cli::spool::open::open_spool_at;
use rusqlite::params;
use serde_json::{json, Value};

type Behaviour = Box<dyn Fn(&[Value]) -> Result<(), DeliveryError>>;

struct RecordingSink {
    behaviour: Behaviour,
    calls: RefCell<usize>,
}

impl RecordingSink {
    fn with(behaviour: Behaviour) -> Self {
        Self {
            behaviour,
            calls: RefCell::new(0),
        }
    }
}

impl OperationSink for RecordingSink {
    fn deliver(&self, operations: &[Value]) -> Result<(), DeliveryError> {
        *self.calls.borrow_mut() += 1;
        (self.behaviour)(operations)
    }
}

fn settings() -> Settings {
    Settings {
        endpoint: "http://127.0.0.1:9".to_owned(),
        max_queued_operations: 10_000,
        max_dead_letter_operations: 1_000,
        http_timeout: Duration::from_secs(3),
    }
}

fn fast_limits() -> DrainLimits {
    DrainLimits {
        max_round: Duration::from_secs(30),
        max_stalled_flushes: 5,
        stall_pause: Duration::from_millis(1),
    }
}

fn paths(root: &Path) -> SpoolPaths {
    SpoolPaths {
        spool: root.join("var").join("spool.sqlite3"),
        flush_lock: root.join("var").join("flush-spool.lock"),
    }
}

fn queue_operations(spool: &Path, count: usize) {
    let conn = open_spool_at(spool).unwrap_or_else(|_| panic!("open spool"));
    for index in 0..count {
        let id = format!("op-{index:03}");
        let created_at = format!("2026-01-01T00:{:02}:{:02}Z", index / 60, index % 60);
        let payload = json!({"operationId": id, "kind": "k", "occurredAt": created_at, "data": {}});
        conn.execute(
            "INSERT INTO queue(operation_id,payload,created_at,attempts) VALUES(?,?,?,0)",
            params![id, payload.to_string(), created_at],
        )
        .unwrap();
    }
}

fn queued(spool: &Path) -> i64 {
    let conn = open_spool_at(spool).unwrap_or_else(|_| panic!("open spool"));
    conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn drains_every_batch_until_the_queue_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    queue_operations(&paths.spool, 120);
    let sink = RecordingSink::with(Box::new(|_| Ok(())));

    let outcome = drain_spool(&paths, &settings(), &sink, &fast_limits()).unwrap();

    assert_eq!(outcome.status, DrainStatus::Drained);
    assert_eq!(outcome.delivered, 120);
    assert_eq!(outcome.remaining, 0);
    assert_eq!(*sink.calls.borrow(), 3);
    assert_eq!(queued(&paths.spool), 0);
}

#[test]
fn stops_after_five_stalled_flushes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    queue_operations(&paths.spool, 3);
    let sink = RecordingSink::with(Box::new(|_| {
        Err(DeliveryError::Rejected {
            status: 500,
            reason: "down".to_owned(),
        })
    }));

    let outcome = drain_spool(&paths, &settings(), &sink, &fast_limits()).unwrap();

    assert_eq!(outcome.status, DrainStatus::Stalled);
    assert_eq!(outcome.remaining, 3);
    assert_eq!(*sink.calls.borrow(), 5);
}

#[test]
fn poison_operations_are_dead_lettered_and_neighbours_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    queue_operations(&paths.spool, 4);
    let sink = RecordingSink::with(Box::new(|operations| {
        if operations
            .iter()
            .any(|operation| operation["operationId"] == "op-002")
        {
            return Err(DeliveryError::Rejected {
                status: 400,
                reason: "bad".to_owned(),
            });
        }
        Ok(())
    }));

    let outcome = drain_spool(&paths, &settings(), &sink, &fast_limits()).unwrap();

    assert_eq!(outcome.status, DrainStatus::Drained);
    assert_eq!(outcome.delivered, 3);
    let conn = open_spool_at(&paths.spool).unwrap_or_else(|_| panic!("open spool"));
    let parked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dead_letter WHERE operation_id='op-002'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parked, 1);
}

#[test]
fn another_flusher_holding_the_lock_skips_the_round() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    queue_operations(&paths.spool, 2);
    let _python_flusher = try_exclusive(&paths.flush_lock).unwrap().unwrap();
    let sink = RecordingSink::with(Box::new(|_| Ok(())));

    let outcome = drain_spool(&paths, &settings(), &sink, &fast_limits()).unwrap();

    assert_eq!(outcome.status, DrainStatus::LockHeld);
    assert_eq!(*sink.calls.borrow(), 0);
    assert_eq!(queued(&paths.spool), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn requests_coalesce_into_one_pending_round() {
    let (handle, receiver) = channel();
    handle.request_flush();
    handle.clone().request_flush();
    handle.request_flush();

    assert_eq!(
        receiver.next_trigger(Duration::from_secs(5)).await,
        DrainTrigger::Requested
    );
    assert_eq!(
        receiver.next_trigger(Duration::from_millis(20)).await,
        DrainTrigger::SafetyNet
    );
}
