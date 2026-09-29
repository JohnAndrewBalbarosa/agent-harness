//! The hub database is opened so that no statement can change it: the file handle is
//! read-only, and `query_only` (set after TEMP views exist) also blocks TEMP writes.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags};

use crate::failure::{CliFailure, CliResult};

/// The hub writes in WAL mode; readers wait briefly instead of failing on a checkpoint.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub fn open_read_only(path: &Path) -> CliResult<Connection> {
    if !path.is_file() {
        return Err(CliFailure::refusal(format!(
            "Hub database not found: {}",
            path.display()
        )));
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("unable to open hub database {} read-only", path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// After this, even the TEMP schema rejects writes for the rest of the connection.
pub fn forbid_writes(conn: &Connection) -> CliResult<()> {
    conn.execute_batch("PRAGMA query_only=ON")?;
    Ok(())
}
