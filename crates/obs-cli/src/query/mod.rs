//! `obs query`: read-only SQL over the hub SQLite file.
//! - `read_only_connection`  open with `SQLITE_OPEN_READ_ONLY`, then `PRAGMA query_only=ON`
//! - `project_views`         TEMP views reproducing the per-project Postgres schema views
//! - `statement`             single read-only statement, bounded row collection
//! - `render`                JSON array of row objects or an aligned text table

pub mod project_views;
pub mod read_only_connection;
pub mod render;
pub mod statement;
