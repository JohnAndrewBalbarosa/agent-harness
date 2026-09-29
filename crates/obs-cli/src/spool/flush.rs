//! obs.py:284-329 `flush` / `_deliver_with_isolation`: deliver the 50 oldest queued operations.
//!
//! - HTTP 400/413 (poison): bisect until the single bad operation is found, dead-letter it,
//!   and keep delivering its neighbours.
//! - Unreachable hub (`URLError`): bisect as well; a single operation that already had 4+
//!   attempts is dead-lettered, otherwise the flush aborts.
//! - Anything else aborts the flush; every claimed operation is charged one attempt.

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::failure::CliResult;
use crate::hub_client::{post_operations, DeliveryError};
use crate::python_compat::truncate_chars;
use crate::settings::Settings;
use crate::spool::begin_write;
use crate::spool::dead_letter::park_in_dead_letter;

const FLUSH_BATCH_SIZE: i64 = 50;
const POISON_STATUSES: [u16; 2] = [400, 413];
const NETWORK_ATTEMPTS_BEFORE_DEAD_LETTER: i64 = 4;
const MAX_ERROR_CHARS: usize = 300;

/// Where flushed operations go; the hub in production, a recorder in tests.
pub trait OperationSink {
    fn deliver(&self, operations: &[Value]) -> Result<(), DeliveryError>;
}

pub struct HubSink<'a> {
    pub settings: &'a Settings,
}

impl OperationSink for HubSink<'_> {
    fn deliver(&self, operations: &[Value]) -> Result<(), DeliveryError> {
        post_operations(self.settings, operations)
    }
}

/// One `queue` row as claimed for delivery.
pub struct QueuedOperation {
    pub operation_id: String,
    pub payload: String,
    pub created_at: String,
    pub attempts: i64,
}

/// A failure that stops the whole flush (obs.py `except Exception` in `flush`).
struct FlushAbort(String);

impl From<rusqlite::Error> for FlushAbort {
    fn from(error: rusqlite::Error) -> Self {
        Self(error.to_string())
    }
}

/// Mental model: claim the oldest batch → deliver with isolation → on an abort, charge one
/// attempt to every claimed row. Returns how many operations the hub accepted.
pub fn flush_oldest_batch(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
) -> CliResult<usize> {
    let batch = claim_oldest_batch(conn)?;
    if batch.is_empty() {
        return Ok(0);
    }

    match deliver_with_isolation(conn, settings, sink, &batch) {
        Ok(delivered) => Ok(delivered),
        Err(abort) => {
            record_failed_attempt(conn, &batch, &abort.0)?;
            Ok(0)
        }
    }
}

fn claim_oldest_batch(conn: &Connection) -> rusqlite::Result<Vec<QueuedOperation>> {
    let mut statement = conn.prepare(
        "SELECT operation_id,payload,created_at,attempts FROM queue ORDER BY created_at LIMIT ?",
    )?;
    let rows = statement.query_map(params![FLUSH_BATCH_SIZE], |row| {
        Ok(QueuedOperation {
            operation_id: row.get(0)?,
            payload: row.get(1)?,
            created_at: row.get(2)?,
            attempts: row.get(3)?,
        })
    })?;
    rows.collect()
}

fn deliver_with_isolation(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    rows: &[QueuedOperation],
) -> Result<usize, FlushAbort> {
    if rows.is_empty() {
        return Ok(0);
    }

    let operations = decode_payloads(rows)?;
    match sink.deliver(&operations) {
        Ok(()) => {
            acknowledge_delivered(conn, rows)?;
            Ok(rows.len())
        }
        Err(error) => isolate_failed_delivery(conn, settings, sink, rows, error),
    }
}

fn decode_payloads(rows: &[QueuedOperation]) -> Result<Vec<Value>, FlushAbort> {
    rows.iter()
        .map(|row| {
            serde_json::from_str(&row.payload).map_err(|error| FlushAbort(error.to_string()))
        })
        .collect()
}

fn isolate_failed_delivery(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    rows: &[QueuedOperation],
    error: DeliveryError,
) -> Result<usize, FlushAbort> {
    match &error {
        DeliveryError::Rejected { status, .. } if is_poison_status(*status) => {
            isolate_poison_operation(conn, settings, sink, rows, &error)
        }
        DeliveryError::Unreachable(_) => {
            isolate_unreachable_operation(conn, settings, sink, rows, &error)
        }
        _ => Err(FlushAbort(error.to_string())),
    }
}

fn isolate_poison_operation(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    rows: &[QueuedOperation],
    error: &DeliveryError,
) -> Result<usize, FlushAbort> {
    match rows {
        [poison] => {
            park_in_dead_letter(conn, settings, poison, &error_text(error))?;
            Ok(0)
        }
        _ => deliver_both_halves(conn, settings, sink, rows),
    }
}

fn isolate_unreachable_operation(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    rows: &[QueuedOperation],
    error: &DeliveryError,
) -> Result<usize, FlushAbort> {
    match rows {
        [stuck] if has_exhausted_network_attempts(stuck) => {
            park_in_dead_letter(conn, settings, stuck, &error_text(error))?;
            Ok(0)
        }
        [_] => Err(FlushAbort(error.to_string())),
        _ => deliver_both_halves(conn, settings, sink, rows),
    }
}

/// obs.py: `left + right`; a failure in the left half stops before the right half.
fn deliver_both_halves(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    rows: &[QueuedOperation],
) -> Result<usize, FlushAbort> {
    let (left, right) = rows.split_at(rows.len() / 2);
    let delivered_left = deliver_with_isolation(conn, settings, sink, left)?;
    let delivered_right = deliver_with_isolation(conn, settings, sink, right)?;
    Ok(delivered_left + delivered_right)
}

#[inline]
fn is_poison_status(status: u16) -> bool {
    POISON_STATUSES.contains(&status)
}

#[inline]
fn has_exhausted_network_attempts(row: &QueuedOperation) -> bool {
    row.attempts >= NETWORK_ATTEMPTS_BEFORE_DEAD_LETTER
}

#[inline]
fn error_text(error: &DeliveryError) -> String {
    truncate_chars(&error.to_string(), MAX_ERROR_CHARS)
}

fn acknowledge_delivered(conn: &Connection, rows: &[QueuedOperation]) -> rusqlite::Result<()> {
    let transaction = begin_write(conn)?;
    for row in rows {
        transaction.execute(
            "DELETE FROM queue WHERE operation_id=?",
            params![row.operation_id],
        )?;
    }
    transaction.commit()
}

fn record_failed_attempt(
    conn: &Connection,
    rows: &[QueuedOperation],
    reason: &str,
) -> rusqlite::Result<()> {
    let message = truncate_chars(reason, MAX_ERROR_CHARS);
    let transaction = begin_write(conn)?;
    for row in rows {
        transaction.execute(
            "UPDATE queue SET attempts=attempts+1,last_error=? WHERE operation_id=?",
            params![message, row.operation_id],
        )?;
    }
    transaction.commit()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::*;
    use crate::spool::open::prepare_spool_connection;
    use serde_json::json;

    type Behaviour = Box<dyn Fn(&[Value]) -> Result<(), DeliveryError>>;

    pub(crate) struct RecordingSink {
        behaviour: Behaviour,
        delivered: RefCell<Vec<Value>>,
        calls: RefCell<usize>,
    }

    impl RecordingSink {
        pub(crate) fn accepting() -> Self {
            Self::with(Box::new(|_| Ok(())))
        }

        fn with(behaviour: Behaviour) -> Self {
            Self {
                behaviour,
                delivered: RefCell::new(Vec::new()),
                calls: RefCell::new(0),
            }
        }

        pub(crate) fn delivered_ids(&self) -> Vec<String> {
            self.delivered
                .borrow()
                .iter()
                .map(|operation| {
                    operation["operationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        }
    }

    impl OperationSink for RecordingSink {
        fn deliver(&self, operations: &[Value]) -> Result<(), DeliveryError> {
            *self.calls.borrow_mut() += 1;
            (self.behaviour)(operations)?;
            self.delivered
                .borrow_mut()
                .extend(operations.iter().cloned());
            Ok(())
        }
    }

    pub(crate) fn settings_for_tests() -> Settings {
        Settings {
            endpoint: "http://127.0.0.1:9".to_owned(),
            max_queued_operations: 10_000,
            max_dead_letter_operations: 1_000,
            http_timeout: Duration::from_secs(3),
        }
    }

    fn spool() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        prepare_spool_connection(&conn).unwrap();
        conn
    }

    fn queue(conn: &Connection, id: &str, kind: &str, created_at: &str, attempts: i64) {
        let payload =
            json!({"operationId": id, "kind": kind, "occurredAt": created_at, "data": {}});
        conn.execute(
            "INSERT INTO queue(operation_id,payload,created_at,attempts) VALUES(?,?,?,?)",
            params![id, payload.to_string(), created_at, attempts],
        )
        .unwrap();
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn outage_charges_an_attempt_then_replays() {
        let conn = spool();
        queue(&conn, "op-1", "test.fixture", "2026-01-01T00:00:00Z", 0);
        let offline = RecordingSink::with(Box::new(|_| {
            Err(DeliveryError::Interrupted("offline".to_owned()))
        }));

        assert_eq!(
            flush_oldest_batch(&conn, &settings_for_tests(), &offline).unwrap(),
            0
        );
        let (attempts, last_error): (i64, String) = conn
            .query_row("SELECT attempts,last_error FROM queue", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(attempts, 1);
        assert!(last_error.contains("offline"));

        let online = RecordingSink::accepting();
        assert_eq!(
            flush_oldest_batch(&conn, &settings_for_tests(), &online).unwrap(),
            1
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM queue"), 0);
    }

    #[test]
    fn poison_operation_is_isolated_without_blocking_neighbours() {
        let conn = spool();
        queue(&conn, "bad", "bad.fixture", "2026-01-01T00:00:00Z", 0);
        queue(&conn, "good", "good.fixture", "2026-01-01T00:00:01Z", 0);
        let sink = RecordingSink::with(Box::new(|operations| {
            if operations
                .iter()
                .any(|operation| operation["kind"] == "bad.fixture")
            {
                return Err(DeliveryError::Rejected {
                    status: 400,
                    reason: "permanent fixture failure".to_owned(),
                });
            }
            Ok(())
        }));

        assert_eq!(
            flush_oldest_batch(&conn, &settings_for_tests(), &sink).unwrap(),
            1
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM queue"), 0);
        let reason: String = conn
            .query_row(
                "SELECT last_error FROM dead_letter WHERE operation_id='bad'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "hub returned HTTP 400: permanent fixture failure");
        assert_eq!(sink.delivered_ids(), vec!["good"]);
    }

    #[test]
    fn unreachable_hub_dead_letters_only_after_four_attempts() {
        let conn = spool();
        queue(&conn, "old", "k", "2026-01-01T00:00:00Z", 4);
        queue(&conn, "new", "k", "2026-01-01T00:00:01Z", 1);
        let unreachable = RecordingSink::with(Box::new(|_| {
            Err(DeliveryError::Unreachable(
                "<urlopen error refused>".to_owned(),
            ))
        }));

        assert_eq!(
            flush_oldest_batch(&conn, &settings_for_tests(), &unreachable).unwrap(),
            0
        );

        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM dead_letter WHERE operation_id='old' AND attempts=5"
            ),
            1
        );
        assert_eq!(
            count(&conn, "SELECT attempts FROM queue WHERE operation_id='new'"),
            2
        );
        assert_eq!(*unreachable.calls.borrow(), 3);
    }

    #[test]
    fn server_error_aborts_without_bisecting() {
        let conn = spool();
        queue(&conn, "a", "k", "2026-01-01T00:00:00Z", 0);
        queue(&conn, "b", "k", "2026-01-01T00:00:01Z", 0);
        let failing = RecordingSink::with(Box::new(|_| {
            Err(DeliveryError::Rejected {
                status: 500,
                reason: "boom".to_owned(),
            })
        }));

        assert_eq!(
            flush_oldest_batch(&conn, &settings_for_tests(), &failing).unwrap(),
            0
        );
        assert_eq!(*failing.calls.borrow(), 1);
        assert_eq!(
            count(&conn, "SELECT count(*) FROM queue WHERE attempts=1"),
            2
        );
    }

    #[test]
    fn delivers_at_most_fifty_oldest() {
        let conn = spool();
        for index in 0..60 {
            queue(
                &conn,
                &format!("op-{index:02}"),
                "k",
                &format!("2026-01-01T00:00:{index:02}Z"),
                0,
            );
        }
        let sink = RecordingSink::accepting();

        assert_eq!(
            flush_oldest_batch(&conn, &settings_for_tests(), &sink).unwrap(),
            50
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM queue"), 10);
        assert_eq!(
            sink.delivered_ids().first().map(String::as_str),
            Some("op-00")
        );
    }
}
