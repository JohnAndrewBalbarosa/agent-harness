//! core/usage/obs_client.py `enqueue_usage`: `obs.enqueue([obs.operation("turn-usage.record", r) ...],
//! flush_now=False)`. Reuses the hook's spool writer (same envelope, redaction, compact ASCII JSON,
//! queue cap). The flush is the caller's drain signal, never a spawned process.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use harness_hook::consumers::usage_fallback::spool_enqueue::enqueue_usage;
use serde_json::Value;

/// Enqueues every record; returns how many were written. An empty batch never touches the spool.
///
/// Deviation: obs.py wrote the batch in one transaction; here each record is its own transaction
/// (same rows, same cap), so a mid-batch failure keeps the records already written.
pub fn enqueue_usage_records(
    spool: &Path,
    records: &[Value],
    env: &BTreeMap<String, String>,
) -> anyhow::Result<usize> {
    for record in records {
        enqueue_usage(spool, record, env)
            .with_context(|| format!("unable to enqueue turn usage into {}", spool.display()))?;
    }
    Ok(records.len())
}
