//! Only supported successful Read tools establish coverage; a shell path mention is not a read.
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use super::reader::read_document;
use super::state::{sha256_bytes, CachePaths, ContentObservation};
use crate::hook_context::HookContext;

pub fn observe_tool_read(context: &HookContext, cache: &CachePaths, root: &Path) -> io::Result<()> {
    let started = Instant::now();
    let payload = &context.native_payload;
    if payload.get("tool_name").and_then(Value::as_str) != Some("Read") {
        return Ok(());
    }
    let response = &payload["tool_response"];
    if response.is_null()
        || response.get("is_error").and_then(Value::as_bool) == Some(true)
        || response.get("isError").and_then(Value::as_bool) == Some(true)
        || response.get("error").is_some_and(|value| !value.is_null())
    {
        return Ok(());
    }
    let input = &payload["tool_input"];
    let Some(path) = input.get("file_path").and_then(Value::as_str) else {
        return Ok(());
    };
    let path = PathBuf::from(path);
    let absolute = if path.is_absolute() {
        path
    } else {
        Path::new(context.event.cwd.as_deref().unwrap_or_default()).join(path)
    };
    let absolute = absolute.canonicalize()?;
    let Ok(relative) = absolute.strip_prefix(root) else {
        return Ok(());
    };
    if !relative
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        return Ok(());
    }
    let bytes = read_document(root, relative)?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let key = super::policy::normalized_path(relative);
    let hash = sha256_bytes(&bytes);
    // Exact returned content is evidence. Tool defaults, offsets and filenames are not.
    let returned = response
        .as_str()
        .or_else(|| response.get("content").and_then(Value::as_str))
        .or_else(|| {
            response
                .get("file")
                .and_then(|file| file.get("content"))
                .and_then(Value::as_str)
        });
    let interval = returned
        .filter(|value| !value.is_empty())
        .and_then(|value| {
            text.find(value).map(|offset| {
                (
                    text[..offset].chars().count(),
                    text[..offset].chars().count() + value.chars().count(),
                )
            })
        });
    let coverage = if returned == Some(text.as_str()) {
        "full"
    } else if interval.is_some() {
        "partial"
    } else {
        "unknown"
    };
    cache.update_session(|state| {
        let previous = state.observations.get(&key);
        if !previous.is_some_and(|prior| prior.hash == hash && prior.coverage == "full") {
            state.observations.insert(
                key.clone(),
                ContentObservation {
                    hash: hash.clone(),
                    source: "tool_read".into(),
                    coverage: coverage.into(),
                    start_character: interval.map(|range| range.0),
                    end_character: interval.map(|range| range.1),
                    ranges: interval.into_iter().collect(),
                },
            );
        }
        Ok(())
    })?;
    super::record(
        context,
        "markdown.observed",
        "succeeded",
        started,
        json!({"source": "tool_read", "coverage": coverage, "versionHash": hash}),
    );
    super::telemetry::record_observation(
        context,
        root,
        &key,
        &hash,
        bytes.len(),
        coverage,
        "tool_read",
    )?;
    Ok(())
}
