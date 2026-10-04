//! Explicit bounded Markdown reads, with versioned coverage rather than an assumed full load.
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::index::MarkdownIndex;
use super::policy::{normalized_path, resolve_document};
use super::state::{sha256_bytes, CachePaths, ContentObservation};
use crate::hook_context::HookContext;

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub path: String,
    pub hash: String,
    pub coverage: String,
    pub outcome: String,
    pub start_character: usize,
    pub end_character: usize,
    pub total_characters: usize,
    pub byte_size: usize,
    pub content: String,
}

#[derive(Debug)]
pub struct ReadRequest {
    pub files: Vec<PathBuf>,
    pub offset: usize,
    pub max_characters: usize,
    pub force: bool,
}

// Mental model: validate every explicit path → read one bounded version → check prior coverage and
// remaining session budget → return the selected interval, never an automatic deferred batch.
pub fn read_selected(
    context: &HookContext,
    cache: &CachePaths,
    index: &MarkdownIndex,
    request: &ReadRequest,
) -> io::Result<Vec<ReadResult>> {
    let available = index.paths();
    let mut unique = std::collections::HashSet::new();
    let files = request
        .files
        .iter()
        .filter(|path| unique.insert((*path).clone()))
        .collect::<Vec<_>>();
    for path in &files {
        if !available.contains(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Document is absent from the allowed Markdown index",
            ));
        }
        resolve_document(&index.tree.canonical_root, path)?;
    }
    let session_limit = super::configured_limit(context, "AH_MARKDOWN_SESSION_CHARS", 64_000);
    cache.update_session(|state| {
        let mut remaining = request
            .max_characters
            .min(session_limit.saturating_sub(state.supplied_characters));
        let mut results = Vec::new();
        for relative in files {
            let bytes = read_document(&index.tree.canonical_root, relative)?;
            let hash = sha256_bytes(&bytes);
            let byte_size = bytes.len();
            let text = String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let total = text.chars().count();
            if request.offset > total {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Character offset exceeds document length",
                ));
            }
            let path = normalized_path(relative);
            let start = request.offset;
            let end = start.saturating_add(remaining).min(total);
            let previous = state.observations.get(&path);
            let requested_end = start.saturating_add(request.max_characters).min(total);
            let covered = previous.is_some_and(|prior| prior.covers(&hash, start, requested_end));
            let outcome = if covered && !request.force {
                "already_observed"
            } else if remaining == 0 && total > start {
                "budget_exhausted"
            } else {
                "emitted"
            };
            let content = if outcome == "emitted" {
                text.chars()
                    .skip(start)
                    .take(end - start)
                    .collect::<String>()
            } else {
                String::new()
            };
            let coverage = if start == 0 && end == total {
                "full"
            } else {
                "partial"
            };
            if outcome == "emitted" {
                remaining -= content.chars().count();
                state.supplied_characters += content.chars().count();
                let mut observation = previous
                    .filter(|prior| prior.hash == hash)
                    .cloned()
                    .unwrap_or(ContentObservation {
                        hash: hash.clone(),
                        source: "explicit_read".into(),
                        coverage: "partial".into(),
                        start_character: Some(start),
                        end_character: Some(end),
                        ranges: Vec::new(),
                    });
                observation.source = "explicit_read".into();
                observation.start_character = Some(start);
                observation.end_character = Some(end);
                observation.include_range(start, end, total);
                state.observations.insert(path.clone(), observation);
            }
            results.push(ReadResult {
                path,
                hash,
                coverage: coverage.into(),
                outcome: outcome.into(),
                start_character: start,
                end_character: end,
                total_characters: total,
                byte_size,
                content,
            });
        }
        Ok(results)
    })
}

pub fn read_document(root: &Path, relative: &Path) -> io::Result<Vec<u8>> {
    let path = resolve_document(root, relative)?;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Markdown file exceeds the 2 MiB read bound",
        ));
    }
    Ok(bytes)
}
