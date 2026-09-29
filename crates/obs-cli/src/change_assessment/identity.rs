//! obs.py:407 `_change_identity`: which project, prompt, and delivery stream a change belongs to.
//! The counter is scoped by `(project_id, delivery_stream)`; assessments by all three.

use std::path::Path;

use rusqlite::Connection;

use crate::failure::{CliFailure, CliResult};
use crate::git::git_value;
use crate::project_resolution::find_marker_from_cwd;
use crate::python_compat::{python_uuid_text, truncate_chars};
use crate::spool::context::active_prompt_id;

const DEFAULT_DELIVERY_STREAM: &str = "project-default";
const MAX_DELIVERY_STREAM_CHARS: usize = 240;

pub struct ChangeIdentity {
    pub project_id: String,
    pub prompt_id: String,
    pub delivery_stream: String,
}

/// Mental model: marker → project id; explicit or hook-recorded prompt id (must be a UUID);
/// explicit stream, else the current git branch, else `project-default`.
pub fn resolve_change_identity(
    conn: &Connection,
    explicit_prompt_id: Option<&str>,
    explicit_stream: Option<&str>,
) -> CliResult<ChangeIdentity> {
    let marker = find_marker_from_cwd()?
        .ok_or_else(|| CliFailure::refusal("No observability.project.toml found."))?;
    let project_id = marker.project_id_text()?;
    let prompt_id = resolve_prompt_id(conn, &project_id, explicit_prompt_id)?;
    let delivery_stream = resolve_delivery_stream(marker.root(), explicit_stream)?;

    Ok(ChangeIdentity {
        project_id,
        prompt_id,
        delivery_stream,
    })
}

fn resolve_prompt_id(
    conn: &Connection,
    project_id: &str,
    explicit: Option<&str>,
) -> CliResult<String> {
    let candidate = match non_empty(explicit) {
        Some(prompt_id) => prompt_id.to_owned(),
        None => active_prompt_id(conn, project_id)?.unwrap_or_default(),
    };
    let trimmed = candidate.trim();

    if trimmed.is_empty() {
        return Err(CliFailure::refusal(
            "No active prompt ID is available; pass --prompt-id explicitly.",
        ));
    }
    python_uuid_text(trimmed).ok_or_else(|| {
        CliFailure::refusal(
            "Prompt ID must be a UUID; use the active hook-provided prompt ID or pass a UUID.",
        )
    })
}

fn resolve_delivery_stream(project_root: &Path, explicit: Option<&str>) -> CliResult<String> {
    let requested = match non_empty(explicit) {
        Some(stream) => stream.to_owned(),
        None => current_branch_or_default(project_root),
    };
    let stream = requested.trim();

    if stream.is_empty() {
        return Err(CliFailure::refusal("Delivery stream cannot be empty."));
    }
    Ok(truncate_chars(stream, MAX_DELIVERY_STREAM_CHARS))
}

fn current_branch_or_default(project_root: &Path) -> String {
    let branch = git_value(project_root, &["branch", "--show-current"]);
    if branch.is_empty() {
        DEFAULT_DELIVERY_STREAM.to_owned()
    } else {
        branch
    }
}

/// Python truthiness of an optional CLI string (`None` and `""` are both falsy).
#[inline]
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_stream_is_trimmed_and_bounded() {
        let long = format!("  {}  ", "s".repeat(300));
        let stream = resolve_delivery_stream(Path::new("."), Some(&long)).unwrap();
        assert_eq!(stream.chars().count(), 240);
    }

    #[test]
    fn whitespace_stream_is_refused() {
        assert!(matches!(
            resolve_delivery_stream(Path::new("."), Some("   ")),
            Err(CliFailure::Refusal(message)) if message == "Delivery stream cannot be empty."
        ));
    }

    #[test]
    fn explicit_prompt_must_be_a_uuid() {
        let conn = Connection::open_in_memory().unwrap();
        crate::spool::open::prepare_spool_connection(&conn).unwrap();

        assert_eq!(
            resolve_prompt_id(&conn, "p", Some("01A0D736-8ED8-76DC-9C20-C342229F9A82")).unwrap(),
            "01a0d736-8ed8-76dc-9c20-c342229f9a82"
        );
        assert!(matches!(
            resolve_prompt_id(&conn, "p", Some("p1")),
            Err(CliFailure::Refusal(_))
        ));
        assert!(matches!(
            resolve_prompt_id(&conn, "p", None),
            Err(CliFailure::Refusal(_))
        ));
    }
}
