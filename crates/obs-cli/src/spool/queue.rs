//! obs.py:226 `enqueue(ops)`: persist first (the spool is the source of truth when the hub is
//! down), keep only the newest `OBS_MAX_QUEUED_OPERATIONS`, then attempt one flush.

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::failure::CliResult;
use crate::python_compat::{python_utc_now, to_python_json, JsonLayout};
use crate::settings::Settings;
use crate::spool::begin_write;
use crate::spool::flush::{flush_oldest_batch, OperationSink};

/// Mental model: store → trim → deliver the oldest batch (delivery outcome is not reported).
pub fn enqueue_and_flush(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    operations: &[Value],
) -> CliResult<()> {
    persist_operations(conn, settings, operations)?;
    flush_oldest_batch(conn, settings, sink)?;
    Ok(())
}

/// Durable enqueue without delivery (obs.py `enqueue(..., flush_now=False)`); a flusher delivers later.
pub fn persist_operations(
    conn: &Connection,
    settings: &Settings,
    operations: &[Value],
) -> CliResult<()> {
    let transaction = begin_write(conn)?;

    for operation in operations {
        insert_queued_operation(&transaction, operation)?;
    }
    trim_queue_to_cap(&transaction, settings.max_queued_operations)?;

    transaction.commit()?;
    Ok(())
}

fn insert_queued_operation(conn: &Connection, operation: &Value) -> rusqlite::Result<()> {
    let operation_id = operation
        .get("operationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = to_python_json(operation, JsonLayout::Compact, true);

    conn.execute(
        "INSERT OR IGNORE INTO queue(operation_id,payload,created_at) VALUES(?,?,?)",
        params![operation_id, payload, python_utc_now()],
    )?;
    Ok(())
}

/// Drops the oldest rows beyond the cap (newest `cap` rows survive).
fn trim_queue_to_cap(conn: &Connection, cap: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM queue WHERE operation_id IN (SELECT operation_id FROM queue ORDER BY created_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
        params![cap],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spool::flush::tests::{settings_for_tests, RecordingSink};
    use crate::spool::open::prepare_spool_connection;
    use serde_json::json;

    #[test]
    fn enqueued_operation_is_stored_then_delivered() {
        let conn = Connection::open_in_memory().unwrap();
        prepare_spool_connection(&conn).unwrap();
        let sink = RecordingSink::accepting();
        let operation = json!({"operationId": "op-1", "kind": "k", "occurredAt": "2026-01-01T00:00:00Z", "data": {}});

        enqueue_and_flush(&conn, &settings_for_tests(), &sink, &[operation]).unwrap();

        assert_eq!(sink.delivered_ids(), vec!["op-1"]);
        let remaining: i64 = conn
            .query_row("SELECT count(*) FROM queue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn queue_is_trimmed_to_the_newest_rows() {
        let conn = Connection::open_in_memory().unwrap();
        prepare_spool_connection(&conn).unwrap();
        for index in 0..5 {
            conn.execute(
                "INSERT INTO queue(operation_id,payload,created_at) VALUES(?,?,?)",
                params![
                    format!("op-{index}"),
                    "{}",
                    format!("2026-01-01T00:00:0{index}Z")
                ],
            )
            .unwrap();
        }

        trim_queue_to_cap(&conn, 2).unwrap();

        let mut statement = conn
            .prepare("SELECT operation_id FROM queue ORDER BY created_at")
            .unwrap();
        let ids: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(ids, vec!["op-3", "op-4"]);
    }
}
