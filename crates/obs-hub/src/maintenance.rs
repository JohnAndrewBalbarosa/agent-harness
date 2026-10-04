//! Daily storage upkeep (server.mjs:114-139 without partitions or advisory locks):
//! roll up the last two days, expire payloads, keep 30 days of events and 365 days of rollups,
//! then give freed pages back to the filesystem.

use anyhow::Result;
use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::store::Database;

const EVENT_RETENTION_DAYS: i64 = 30;
const ROLLUP_RETENTION_DAYS: i64 = 365;
const ROLLUP_LOOKBACK_DAYS: i64 = 2;

#[derive(Debug, Serialize)]
pub struct MaintenanceReport {
    pub rollup_rows: usize,
    pub expired_payloads: usize,
    pub expired_events: usize,
    pub expired_rollups: usize,
    pub expired_markdown_batches: usize,
}

/// Runs maintenance and logs the outcome instead of failing the caller (hub keeps serving).
pub async fn run_logged(database: &Database) {
    match run(database).await {
        Ok(report) => tracing::info!(
            event = "hub.maintenance.completed",
            rollup_rows = report.rollup_rows,
            expired_payloads = report.expired_payloads,
            expired_events = report.expired_events,
            expired_rollups = report.expired_rollups,
            expired_markdown_batches = report.expired_markdown_batches,
            "storage maintenance completed"
        ),
        Err(error) => tracing::error!(
            event = "hub.maintenance.failed",
            error = %format!("{error:#}"),
            "storage maintenance failed"
        ),
    }
}

// Mental model:
// refresh recent rollups → delete what aged out → reclaim space and checkpoint the WAL
pub async fn run(database: &Database) -> Result<MaintenanceReport> {
    database
        .write(|connection| {
            let report = apply_retention(connection)?;
            reclaim_space(connection)?;
            Ok(report)
        })
        .await
}

fn apply_retention(connection: &mut Connection) -> Result<MaintenanceReport> {
    let transaction = connection.transaction()?;
    let today = day_start_utc(0);

    let report = MaintenanceReport {
        rollup_rows: refresh_recent_rollups(&transaction, &today)?,
        expired_payloads: transaction.execute(
            "DELETE FROM event_payloads WHERE expires_at < ?1",
            params![obs_common::now_utc_micros()],
        )?,
        expired_events: transaction.execute(
            "DELETE FROM events WHERE occurred_at < ?1",
            params![days_before_now(EVENT_RETENTION_DAYS)],
        )?,
        expired_rollups: transaction.execute(
            "DELETE FROM daily_rollups WHERE day < ?1",
            params![date_days_before_today(ROLLUP_RETENTION_DAYS)],
        )?,
        expired_markdown_batches: expire_markdown_metadata(&transaction)?,
    };

    transaction.commit()?;
    Ok(report)
}

fn expire_markdown_metadata(connection: &Connection) -> Result<usize> {
    let deleted = connection.execute(
        "DELETE FROM markdown_context_batches WHERE batch_id IN
         (SELECT batch_id FROM markdown_context_batches WHERE recorded_at < ?1 ORDER BY recorded_at LIMIT 10000)",
        params![days_before_now(EVENT_RETENTION_DAYS)],
    )?;
    connection.execute("DELETE FROM markdown_document_versions WHERE version_id IN
        (SELECT v.version_id FROM markdown_document_versions v WHERE NOT EXISTS
         (SELECT 1 FROM markdown_context_observations o WHERE o.version_id=v.version_id) LIMIT 10000)", [])?;
    connection.execute("DELETE FROM markdown_documents WHERE document_id IN
        (SELECT d.document_id FROM markdown_documents d WHERE NOT EXISTS
         (SELECT 1 FROM markdown_document_versions v WHERE v.document_id=d.document_id) LIMIT 10000)", [])?;
    Ok(deleted)
}

fn refresh_recent_rollups(connection: &Connection, today_start: &str) -> Result<usize> {
    let window_start = day_start_utc(ROLLUP_LOOKBACK_DAYS);
    Ok(connection.execute(
        "INSERT INTO daily_rollups(project_id,day,event_definition_id,severity,outcome,event_count)
         SELECT x.project_id,substr(e.occurred_at,1,10),e.event_definition_id,e.severity,COALESCE(e.outcome,0),count(*)
         FROM events e JOIN executions x ON x.execution_id=e.execution_id
         WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2
         GROUP BY x.project_id,substr(e.occurred_at,1,10),e.event_definition_id,e.severity,COALESCE(e.outcome,0)
         ON CONFLICT(project_id,day,event_definition_id,severity,outcome) DO UPDATE SET event_count=excluded.event_count",
        params![window_start, today_start],
    )?)
}

fn reclaim_space(connection: &Connection) -> Result<()> {
    for pragma in [
        "PRAGMA incremental_vacuum",
        "PRAGMA optimize",
        "PRAGMA wal_checkpoint(TRUNCATE)",
    ] {
        run_pragma_to_completion(connection, pragma)?;
    }
    Ok(())
}

/// Some pragmas return rows and do their work while being stepped, so drain them fully.
fn run_pragma_to_completion(connection: &Connection, pragma: &str) -> Result<()> {
    let mut statement = connection.prepare(pragma)?;
    let mut rows = statement.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// Midnight UTC `days` days before today, as canonical timestamp text.
#[inline]
fn day_start_utc(days: i64) -> String {
    format!(
        "{}T00:00:00.000000Z",
        Utc::now().date_naive() - Duration::days(days)
    )
}

#[inline]
fn days_before_now(days: i64) -> String {
    obs_common::format_utc_micros(&(Utc::now() - Duration::days(days)))
}

#[inline]
fn date_days_before_today(days: i64) -> String {
    (Utc::now().date_naive() - Duration::days(days)).to_string()
}
