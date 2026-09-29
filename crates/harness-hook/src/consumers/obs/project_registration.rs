//! Which project a hook belongs to (hook.py `project_config` → `register_project` / `global_project`).
//!
//! A marker (`observability.project.toml`) found walking up from the resolved cwd registers that
//! project with its path and origin-remote alias hashes; otherwise the synthetic global project is used.
//! No marker is ever created here.

use std::path::{Path, PathBuf};

use obs_cli::git::git_value;
use obs_cli::project_resolution::{find_project_marker, ProjectMarker};
use obs_cli::python_compat::uuid5_url_text;
use serde_json::{json, Value};

use super::fingerprints::sha256_hex;
use super::operations::operation;
use super::spool_store::cli_failure;

const GLOBAL_PROJECT_NAME: &str = "Codex Global Sessions";
const GLOBAL_PROJECT_SLUG: &str = "codex-global-sessions";
const GLOBAL_PROJECT_KEY: &str = "codex-observability:global-agent-project";
const PATH_ALIAS_KIND: i64 = 2;
const REMOTE_ALIAS_KIND: i64 = 1;

pub struct ProjectIdentity {
    /// `project["id"]` as loaded (a string in every real marker).
    pub id: Value,
    /// `str(project["id"])` for f-strings and the `context.project_id` column.
    pub id_text: String,
    /// `found[0].parent`: the marker directory, `None` for the global project.
    pub marker_root: Option<PathBuf>,
}

impl ProjectIdentity {
    /// hook.py `"project" if found else "global"`.
    #[inline]
    pub fn scope_label(&self) -> &'static str {
        if self.marker_root.is_some() {
            "project"
        } else {
            "global"
        }
    }
}

pub struct ProjectRegistration {
    pub project: ProjectIdentity,
    /// The `project.register` operation (enqueued first, in the hook's single transaction).
    pub operation: Value,
}

pub fn register(cwd_path: &Path) -> anyhow::Result<ProjectRegistration> {
    match find_project_marker(cwd_path).map_err(cli_failure)? {
        Some(marker) => register_marked_project(&marker),
        None => Ok(register_global_project()),
    }
}

/// obs.py:719 `register_project(config_path, config)`.
fn register_marked_project(marker: &ProjectMarker) -> anyhow::Result<ProjectRegistration> {
    let root = marker.root();
    let id = marker.required_field("id").map_err(cli_failure)?;
    let name = marker.required_field("name").map_err(cli_failure)?;
    let slug = marker.required_field("slug").map_err(cli_failure)?;

    let data = json!({
        "projectId": id.to_json(),
        "displayName": name.to_json(),
        "slug": slug.to_json(),
        "aliases": project_aliases(root),
    });
    Ok(ProjectRegistration {
        project: ProjectIdentity {
            id: id.to_json(),
            id_text: id.to_python_str(),
            marker_root: Some(root.to_path_buf()),
        },
        operation: operation("project.register", data, None),
    })
}

/// `[{"kind": 2, "hash": digest(str(root).lower())}]` plus the origin remote when there is one.
fn project_aliases(root: &Path) -> Value {
    let path_text = root.to_string_lossy().to_lowercase();
    let mut aliases = vec![json!({"kind": PATH_ALIAS_KIND, "hash": sha256_hex(&path_text)})];

    let remote = git_value(root, &["remote", "get-url", "origin"]);
    if !remote.is_empty() {
        aliases
            .push(json!({"kind": REMOTE_ALIAS_KIND, "hash": sha256_hex(&remote.to_lowercase())}));
    }
    Value::Array(aliases)
}

/// hook.py `global_project(obs_client)`.
fn register_global_project() -> ProjectRegistration {
    let project_id = uuid5_url_text(GLOBAL_PROJECT_KEY);
    let data = json!({
        "projectId": project_id,
        "displayName": GLOBAL_PROJECT_NAME,
        "slug": GLOBAL_PROJECT_SLUG,
        "aliases": [],
    });
    ProjectRegistration {
        project: ProjectIdentity {
            id: Value::String(project_id.clone()),
            id_text: project_id,
            marker_root: None,
        },
        operation: operation("project.register", data, None),
    }
}
