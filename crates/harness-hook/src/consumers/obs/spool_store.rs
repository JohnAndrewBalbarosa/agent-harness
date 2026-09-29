//! The obs.py spool as the hook uses it (obs.py:159 `db()`, :226 `enqueue`, :58 `secret_key`).
//!
//! Python writers use the same file concurrently, so: WAL + 5 s busy timeout and obs.py's verbatim
//! `CREATE ... IF NOT EXISTS` (obs-cli `prepare_spool_connection`), the `context` column migrations
//! obs.py runs on every open, short IMMEDIATE write transactions, and the same `var/hmac.key`.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use obs_cli::failure::CliFailure;
use obs_cli::python_compat::{parse_python_int, python_utc_now, to_python_json, JsonLayout};
use obs_cli::spool::begin_write;
use obs_cli::spool::open::open_spool_at;
use rusqlite::{params, Connection};
use serde_json::Value;

use super::fingerprints::{hmac_sha256_hex, load_or_create_key};
use crate::hook_context::HookContext;

/// Same override the harness Python usage client and obs-rs honour.
const SPOOL_FILE_ENV: &str = "AH_OBS_SPOOL";
const QUEUE_CAP_ENV: &str = "OBS_MAX_QUEUED_OPERATIONS";
const DEFAULT_QUEUE_CAP: i64 = 10_000;
const MIN_QUEUE_CAP: i64 = 100;
const CLIENT_DIRECTORY: [&str; 2] = ["tools", "observability-client"];
const SPOOL_FILE_NAME: &str = "spool.sqlite3";
const HMAC_KEY_FILE_NAME: &str = "hmac.key";

/// obs.py:183-195, in obs.py's order: (column, ALTER statement).
const CONTEXT_MIGRATIONS: [(&str, &str); 6] = [
    (
        "prompt_ordinal",
        "ALTER TABLE context ADD COLUMN prompt_ordinal INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "last_prompt_id",
        "ALTER TABLE context ADD COLUMN last_prompt_id TEXT",
    ),
    (
        "native_turn",
        "ALTER TABLE context ADD COLUMN native_turn TEXT",
    ),
    (
        "wait_count",
        "ALTER TABLE context ADD COLUMN wait_count INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "background_count",
        "ALTER TABLE context ADD COLUMN background_count INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "behavior_audit_enabled",
        "ALTER TABLE context ADD COLUMN behavior_audit_enabled INTEGER NOT NULL DEFAULT 1",
    ),
];

/// Where obs.py (vendored at `<harness>/tools/observability-client/obs.py`) keeps its state.
#[derive(Debug, Clone)]
pub struct SpoolLocation {
    pub spool_file: PathBuf,
    /// obs.py `VAR`: holds `hmac.key`, `hook-invocations.jsonl`, `flush-spool.lock`.
    pub var_dir: PathBuf,
    /// `<harness>/tools/observability-client` (hook.py / flush_spool.py directory).
    pub client_dir: PathBuf,
}

impl SpoolLocation {
    /// `AH_OBS_SPOOL` when set, else `<harness>/tools/observability-client/var/spool.sqlite3`.
    pub fn resolve(context: &HookContext) -> Self {
        let client_dir = CLIENT_DIRECTORY
            .iter()
            .fold(context.harness_root.clone(), |path, part| path.join(part));
        let spool_file = context
            .env_value(SPOOL_FILE_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| client_dir.join("var").join(SPOOL_FILE_NAME));
        let var_dir = spool_file
            .parent()
            .map_or_else(|| client_dir.join("var"), Path::to_path_buf);
        Self {
            spool_file,
            var_dir,
            client_dir,
        }
    }

    /// The directory obs-rs treats as `OBS_HUB_DIR` so it opens this same spool (`<dir>/var/spool.sqlite3`).
    #[inline]
    pub fn hub_dir(&self) -> PathBuf {
        self.var_dir
            .parent()
            .map_or_else(|| self.var_dir.clone(), Path::to_path_buf)
    }
}

/// `max(100, int(os.environ.get("OBS_MAX_QUEUED_OPERATIONS", "10000")))`.
pub fn queue_cap(context: &HookContext) -> i64 {
    context
        .env_value(QUEUE_CAP_ENV)
        .and_then(parse_python_int)
        .unwrap_or(DEFAULT_QUEUE_CAP)
        .max(MIN_QUEUE_CAP)
}

pub struct Spool {
    conn: Connection,
    key_path: PathBuf,
    key: OnceCell<Vec<u8>>,
    queue_cap: i64,
}

impl Spool {
    /// obs.py `db()`: open, schema, then the `context` column migrations.
    pub fn open(location: &SpoolLocation, queue_cap: i64) -> anyhow::Result<Self> {
        let conn = open_spool_at(&location.spool_file).map_err(cli_failure)?;
        migrate_context_columns(&conn)?;
        Ok(Self {
            conn,
            key_path: location.var_dir.join(HMAC_KEY_FILE_NAME),
            key: OnceCell::new(),
            queue_cap,
        })
    }

    #[inline]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// obs.py `digest(value, keyed=True)`; the key file is read lazily, once per invocation.
    pub fn keyed_digest(&self, value: &str) -> anyhow::Result<String> {
        if self.key.get().is_none() {
            let key = load_or_create_key(&self.key_path)
                .with_context(|| format!("unable to read {}", self.key_path.display()))?;
            let _ = self.key.set(key);
        }
        let key = self.key.get().map(Vec::as_slice).unwrap_or_default();
        hmac_sha256_hex(key, value)
    }

    /// obs.py `enqueue(ops, flush_now=False)`: insert, then keep only the newest `queue_cap` rows.
    pub fn enqueue(&self, operations: &[Value]) -> anyhow::Result<()> {
        let transaction = begin_write(&self.conn)?;
        for operation in operations {
            insert_queued_operation(&transaction, operation)?;
        }
        transaction.execute(
            "DELETE FROM queue WHERE operation_id IN (SELECT operation_id FROM queue ORDER BY created_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
            params![self.queue_cap],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

/// `json.dumps(op, separators=(",", ":"))` (ensure_ascii) with `created_at = now()`.
fn insert_queued_operation(conn: &Connection, operation: &Value) -> rusqlite::Result<()> {
    let operation_id = operation
        .get("operationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    conn.execute(
        "INSERT OR IGNORE INTO queue(operation_id,payload,created_at) VALUES(?,?,?)",
        params![
            operation_id,
            to_python_json(operation, JsonLayout::Compact, true),
            python_utc_now()
        ],
    )?;
    Ok(())
}

/// A concurrent writer may add the same column between our check and ALTER; that race is benign.
fn migrate_context_columns(conn: &Connection) -> anyhow::Result<()> {
    let existing = context_columns(conn)?;
    for (column, statement) in CONTEXT_MIGRATIONS {
        if existing.iter().any(|name| name == column) {
            continue;
        }
        if let Err(error) = conn.execute(statement, []) {
            if !is_duplicate_column(&error) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

fn context_columns(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare("PRAGMA table_info(context)")?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    names.collect()
}

#[inline]
fn is_duplicate_column(error: &rusqlite::Error) -> bool {
    error.to_string().contains("duplicate column")
}

/// obs-cli failures carry no `std::error::Error` impl; keep their text for the invocation audit.
pub(crate) fn cli_failure(failure: CliFailure) -> anyhow::Error {
    match failure {
        CliFailure::Internal(error) => error,
        other => anyhow::anyhow!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_add_hook_columns_to_a_legacy_context_table() {
        let dir = tempfile::tempdir().unwrap();
        let spool_file = dir.path().join("var").join(SPOOL_FILE_NAME);
        std::fs::create_dir_all(spool_file.parent().unwrap()).unwrap();
        {
            let legacy = Connection::open(&spool_file).unwrap();
            legacy
                .execute(
                    "CREATE TABLE context(native_session TEXT PRIMARY KEY,project_id TEXT NOT NULL,session_id TEXT NOT NULL,prompt_id TEXT,execution_id TEXT,trace_id TEXT,sequence_no INTEGER NOT NULL DEFAULT 0,cwd TEXT NOT NULL,updated_at TEXT NOT NULL)",
                    [],
                )
                .unwrap();
        }
        let location = SpoolLocation {
            spool_file: spool_file.clone(),
            var_dir: spool_file.parent().unwrap().to_path_buf(),
            client_dir: dir.path().to_path_buf(),
        };
        let spool = Spool::open(&location, 100).unwrap();
        let columns = context_columns(spool.conn()).unwrap();
        for (column, _) in CONTEXT_MIGRATIONS {
            assert!(
                columns.iter().any(|name| name == column),
                "missing {column}"
            );
        }
        // A second open is a no-op.
        Spool::open(&location, 100).unwrap();
    }
}
