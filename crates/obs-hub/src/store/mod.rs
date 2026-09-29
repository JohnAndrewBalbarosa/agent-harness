//! Embedded storage: one SQLite file, one writer connection for operation batches and one
//! reader connection for diagnostic queries (WAL lets them run concurrently).

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::Connection;

const SCHEMA_SQL: &str = include_str!("schema.sql");
const CACHE_SIZE_KIB: i64 = 2048;
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Owns both connections; every call runs on the blocking pool so the async thread never waits on disk.
pub struct Database {
    writer: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let writer = open_database(path)?;
        let reader = open_reader(path)?;

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
        })
    }

    /// In-memory database for tests: both handles share one connection.
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        prepare_connection(&connection)?;
        connection.execute_batch(SCHEMA_SQL)?;
        let shared = Arc::new(Mutex::new(connection));

        Ok(Self {
            writer: Arc::clone(&shared),
            reader: shared,
        })
    }

    pub async fn write<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        run_locked(Arc::clone(&self.writer), work).await
    }

    pub async fn read<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        run_locked(Arc::clone(&self.reader), work).await
    }
}

/// Opens (creating when absent) the hub database with pragmas and schema applied.
pub fn open_database(path: &Path) -> Result<Connection> {
    ensure_parent_directory(path)?;

    let connection = Connection::open(path)
        .with_context(|| format!("failed to open database {}", path.display()))?;
    enable_incremental_vacuum_when_new(&connection)?;
    prepare_connection(&connection)?;
    connection
        .execute_batch(SCHEMA_SQL)
        .context("failed to apply hub schema")?;

    Ok(connection)
}

fn open_reader(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)
        .with_context(|| format!("failed to open reader for {}", path.display()))?;
    prepare_connection(&connection)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

fn prepare_connection(connection: &Connection) -> Result<()> {
    let _journal_mode: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "cache_size", -CACHE_SIZE_KIB)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

/// `auto_vacuum` only takes effect before the first table exists.
fn enable_incremental_vacuum_when_new(connection: &Connection) -> Result<()> {
    if is_empty_database(connection)? {
        connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    }
    Ok(())
}

#[inline]
fn is_empty_database(connection: &Connection) -> Result<bool> {
    let table_count: i64 =
        connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))?;
    Ok(table_count == 0)
}

fn ensure_parent_directory(path: &Path) -> Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display())),
        _ => Ok(()),
    }
}

async fn run_locked<T, F>(connection: Arc<Mutex<Connection>>, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        // A panic inside an earlier closure poisons the mutex; rusqlite already rolled back any
        // open transaction while unwinding, so the connection itself is still usable.
        let mut guard = connection.lock().unwrap_or_else(PoisonError::into_inner);
        work(&mut guard)
    })
    .await
    .context("database task panicked")?
}
