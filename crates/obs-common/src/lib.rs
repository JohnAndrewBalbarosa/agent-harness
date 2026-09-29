//! Shared vocabulary for the observability hub and CLI: where data lives,
//! how timestamps are normalized, and how identifiers are validated.

pub mod identifiers;
pub mod paths;
pub mod timestamps;

pub use identifiers::{decode_hex_bytes, normalize_uuid, IdentifierError};
pub use paths::{default_database_path, default_spool_path, harness_home};
pub use timestamps::{
    format_utc_micros, normalize_timestamp, now_utc_micros, to_wire_millis, TimestampError,
};
