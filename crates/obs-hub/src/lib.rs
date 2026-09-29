//! Observability hub: accepts operation batches from agent hooks and CLIs over HTTP and
//! answers diagnostic queries, all backed by one embedded SQLite file.
//!
//! Mental model:
//! client spool → POST /v1/operations → validate batch → apply in one transaction → SQLite
//! diagnostic GET → read-only connection → node-pg-compatible JSON

pub mod config;
pub mod http_errors;
pub mod import_postgres;
pub mod maintenance;
pub mod operations;
pub mod routes;
pub mod store;

pub use config::HubConfig;
pub use routes::build_router;
pub use store::{open_database, Database};
