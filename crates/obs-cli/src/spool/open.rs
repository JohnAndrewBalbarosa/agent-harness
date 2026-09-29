//! Where the spool lives and how it is opened (obs.py:159 `db()`).
//!
//! obs.py keeps its spool at `<directory of obs.py>/var/spool.sqlite3`. Two copies exist:
//! the hub CLI (`~/.codex-shared/tools/observability-hub/var`) that `obs.cmd` runs and that
//! holds the change-assessment counter, and the harness client copy
//! (`~/.agent-harness/tools/observability-client/var`). obs-rs resolves, in order:
//! 1. `AH_OBS_SPOOL` — explicit file override (same variable the harness client honours);
//! 2. `OBS_HUB_DIR`  — directory of the dispatching obs.py (`obs.cmd` sets it to `%~dp0`);
//! 3. the obs-rs executable's own directory when an `obs.py` sits beside it (same rule as obs.py);
//! 4. `obs_common::default_spool_path()` (the harness client spool).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use rusqlite::Connection;

use crate::failure::CliResult;

const SPOOL_FILE_ENV: &str = "AH_OBS_SPOOL";
const HUB_DIRECTORY_ENV: &str = "OBS_HUB_DIR";
const SPOOL_RELATIVE_PATH: [&str; 2] = ["var", "spool.sqlite3"];
const SIBLING_SCRIPT_NAME: &str = "obs.py";
/// Python `sqlite3.connect` default `timeout=5.0`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// obs.py:164-182, verbatim so `sqlite_master` text matches whichever process creates it first.
/// The `ALTER TABLE context` migrations (obs.py:183-195) stay obs.py's responsibility.
const SPOOL_SCHEMA: [&str; 6] = [
    "CREATE TABLE IF NOT EXISTS queue(operation_id TEXT PRIMARY KEY,payload TEXT NOT NULL,created_at TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,last_error TEXT)",
    "CREATE TABLE IF NOT EXISTS dead_letter(operation_id TEXT PRIMARY KEY,payload TEXT NOT NULL,created_at TEXT NOT NULL,failed_at TEXT NOT NULL,attempts INTEGER NOT NULL,last_error TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS context(native_session TEXT PRIMARY KEY,project_id TEXT NOT NULL,session_id TEXT NOT NULL,prompt_id TEXT,execution_id TEXT,trace_id TEXT,last_prompt_id TEXT,native_turn TEXT,prompt_ordinal INTEGER NOT NULL DEFAULT 0,sequence_no INTEGER NOT NULL DEFAULT 0,cwd TEXT NOT NULL,updated_at TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS prompt_change_assessments(
        assessment_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,prompt_id TEXT NOT NULL,
        delivery_stream TEXT NOT NULL,change_kind TEXT NOT NULL,declared_class TEXT NOT NULL,
        effective_class TEXT NOT NULL,classification_reason TEXT NOT NULL DEFAULT '',
        estimated_lines INTEGER NOT NULL DEFAULT 0,estimated_files INTEGER NOT NULL DEFAULT 0,
        actual_lines_added INTEGER NOT NULL DEFAULT 0,actual_lines_deleted INTEGER NOT NULL DEFAULT 0,
        actual_files INTEGER NOT NULL DEFAULT 0,risk_flags TEXT NOT NULL DEFAULT '[]',
        source_urls TEXT NOT NULL DEFAULT '[]',input_tokens INTEGER,output_tokens INTEGER,
        cached_input_tokens INTEGER,reasoning_tokens INTEGER,status TEXT NOT NULL DEFAULT 'planned',
        increments_counter INTEGER NOT NULL DEFAULT 0,package_id TEXT,created_at TEXT NOT NULL,
        completed_at TEXT,UNIQUE(project_id,prompt_id,delivery_stream))",
    "CREATE TABLE IF NOT EXISTS change_packages(
        package_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,delivery_stream TEXT NOT NULL,
        delivery_mode TEXT NOT NULL,commit_sha TEXT,pr_url TEXT,prompt_count INTEGER NOT NULL,
        created_at TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS change_assessments_counter ON prompt_change_assessments(project_id,delivery_stream,increments_counter,package_id)",
];

pub fn open_spool() -> CliResult<Connection> {
    let path = spool_path();
    open_spool_at(&path)
}

pub fn open_spool_at(path: &Path) -> CliResult<Connection> {
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(directory)
            .with_context(|| format!("unable to create spool directory {}", directory.display()))?;
    }

    let conn = Connection::open(path)
        .with_context(|| format!("unable to open spool {}", path.display()))?;
    prepare_spool_connection(&conn)?;
    Ok(conn)
}

/// WAL + busy timeout, then obs.py's idempotent schema.
pub fn prepare_spool_connection(conn: &Connection) -> CliResult<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;

    for statement in SPOOL_SCHEMA {
        conn.execute(statement, [])?;
    }
    Ok(())
}

pub fn spool_path() -> PathBuf {
    explicit_spool_file()
        .or_else(hub_directory_spool)
        .or_else(sibling_script_spool)
        .unwrap_or_else(obs_common::default_spool_path)
}

fn explicit_spool_file() -> Option<PathBuf> {
    non_empty_env_path(SPOOL_FILE_ENV)
}

fn hub_directory_spool() -> Option<PathBuf> {
    non_empty_env_path(HUB_DIRECTORY_ENV).map(|directory| spool_under(&directory))
}

fn sibling_script_spool() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let directory = executable.parent()?;
    directory
        .join(SIBLING_SCRIPT_NAME)
        .is_file()
        .then(|| spool_under(directory))
}

#[inline]
fn spool_under(directory: &Path) -> PathBuf {
    SPOOL_RELATIVE_PATH
        .iter()
        .fold(directory.to_path_buf(), |path, part| path.join(part))
}

#[inline]
fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_idempotent_on_an_existing_spool() {
        let conn = Connection::open_in_memory().unwrap();
        prepare_spool_connection(&conn).unwrap();
        prepare_spool_connection(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('queue','dead_letter','context','prompt_change_assessments','change_packages')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 5);
    }

    #[test]
    fn hub_directory_layout_matches_obs_py_var() {
        assert_eq!(
            spool_under(Path::new("hub")),
            Path::new("hub").join("var").join("spool.sqlite3")
        );
    }
}
