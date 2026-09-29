//! Global GitHub Save Policy bookkeeping shared by `change plan|finish|package`:
//! - `classification`  change classes and their ranking (obs.py:390-404)
//! - `identity`        project + prompt + delivery stream a change belongs to (obs.py:407)
//! - `source_urls`     validated research links (obs.py:426)
//! - `counter`         when a prompt counts, and when the batch must be packaged (obs.py:488, :515)

pub mod classification;
pub mod counter;
pub mod identity;
pub mod source_urls;

use serde_json::Value;

use crate::python_compat::{to_python_json, JsonLayout};

/// obs.py `json.dumps(list)`: how `risk_flags` / `source_urls` are stored in the spool.
pub fn stored_list_text(items: &[String]) -> String {
    let list = Value::Array(items.iter().cloned().map(Value::String).collect());
    to_python_json(&list, JsonLayout::Spaced, true)
}
