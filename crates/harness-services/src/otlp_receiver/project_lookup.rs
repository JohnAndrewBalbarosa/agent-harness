//! core/usage/obs_client.py `spool_project`: the spool `context` table maps a native session to its
//! registered project (read-only lookup; any SQLite failure reads as unknown). `usage_records` turns
//! unknown into the global agent project (receiver.py:74 `... or GLOBAL_PROJECT_ID`).

use std::path::Path;

use harness_hook::consumers::usage_fallback::spool_enqueue::spool_project;

/// `spool_project(native_session)` with Python's falsy empty string treated as missing.
#[inline]
pub fn registered_project(spool: &Path, native_session: &str) -> Option<String> {
    spool_project(spool, native_session).filter(|project| !project.is_empty())
}
