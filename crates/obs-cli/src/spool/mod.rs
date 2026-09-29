//! The local SQLite spool shared with running obs.py processes (obs.py:159-330).
//!
//! - `open`         where the spool lives, WAL + busy timeout, obs.py's `CREATE ... IF NOT EXISTS`
//! - `queue`        enqueue operations, trim to `OBS_MAX_QUEUED_OPERATIONS`, then flush once
//! - `flush`        oldest-50 delivery with bisection of poison/unreachable operations
//! - `dead_letter`  parking operations that can never be delivered
//! - `context`      hook-written session context (active prompt id)
//! - `change_ledger` prompt change assessments and packages (the deterministic counter)

pub mod change_ledger;
pub mod context;
mod dead_letter;
pub mod flush;
pub mod open;
pub mod queue;

use rusqlite::{Connection, Transaction, TransactionBehavior};

/// Python `with conn:` for a write: short, and IMMEDIATE so concurrent writers wait on
/// `busy_timeout` instead of failing on a read-to-write lock upgrade.
pub fn begin_write(conn: &Connection) -> rusqlite::Result<Transaction<'_>> {
    Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
}
