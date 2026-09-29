//! obs.py:271 `_move_to_dead_letter`: park an undeliverable operation, remove it from the
//! queue, and keep only the newest `OBS_MAX_DEAD_LETTER_OPERATIONS` parked rows.

use rusqlite::{params, Connection};

use crate::python_compat::python_utc_now;
use crate::settings::Settings;
use crate::spool::begin_write;
use crate::spool::flush::QueuedOperation;

pub fn park_in_dead_letter(
    conn: &Connection,
    settings: &Settings,
    row: &QueuedOperation,
    reason: &str,
) -> rusqlite::Result<()> {
    let transaction = begin_write(conn)?;

    transaction.execute(
        "INSERT OR REPLACE INTO dead_letter(operation_id,payload,created_at,failed_at,attempts,last_error) VALUES(?,?,?,?,?,?)",
        params![row.operation_id, row.payload, row.created_at, python_utc_now(), row.attempts + 1, reason],
    )?;
    transaction.execute(
        "DELETE FROM queue WHERE operation_id=?",
        params![row.operation_id],
    )?;
    trim_dead_letter_to_cap(&transaction, settings.max_dead_letter_operations)?;

    transaction.commit()
}

fn trim_dead_letter_to_cap(conn: &Connection, cap: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM dead_letter WHERE operation_id IN (SELECT operation_id FROM dead_letter ORDER BY failed_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
        params![cap],
    )?;
    Ok(())
}
