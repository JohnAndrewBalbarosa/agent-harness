//! State shared with still-running Python processes: sidecar file locks, the lifecycle JSONL log,
//! and atomic file replacement. Formats and lock semantics must match `core/locking.py`,
//! `core/lifecycle.py` and the consumers' temp-file-then-replace writes.

pub mod atomic_write;
pub mod file_lock;
pub mod lifecycle_log;
pub mod spool_access;
